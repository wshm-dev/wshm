//! SQLite-backed encrypted secret store.
//!
//! Schema is created at `open()` time. The DB file lives at
//! `$WSHM_HOME/secrets.db` (passed in by the caller). Plaintext values
//! never touch disk — only the AES-GCM ciphertext does.

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::cipher::{aad_for, aad_for_legacy, Cipher, MasterKey};
use super::{Scope, SecretStore};

/// One row of the secrets table — values are returned masked unless the
/// caller explicitly asked for the plaintext via [`SecretStore::reveal`].
#[derive(Clone, Debug, Serialize)]
pub struct SecretRecord {
    pub id: i64,
    pub scope: String,
    pub slug: Option<String>,
    pub key: String,
    pub updated_at: String,
    pub updated_by: Option<i64>,
    /// Always `"••••••••"` from `list()` / `get_record()`. Use
    /// [`SecretStore::reveal`] to obtain the plaintext.
    #[serde(rename = "value")]
    pub masked_value: String,
}

/// SQLite-backed implementation of [`SecretStore`]. Suitable for OSS
/// standalone deployments and single-replica Pro setups.
#[derive(Clone)]
pub struct SqliteSecretStore {
    conn: Arc<Mutex<Connection>>,
    /// Synchronous mirror used by [`get_blocking`] from non-async code.
    sync_conn: Arc<std::sync::Mutex<Connection>>,
    cipher: Arc<Cipher>,
}

impl SqliteSecretStore {
    pub fn open(path: &Path, master_key: &MasterKey) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening secrets db at {}", path.display()))?;
        run_migrations(&conn)?;

        // The `sync_conn` is a SECOND connection on the same DB used for
        // synchronous reads from non-async daemon code (scheduler / poller).
        let sync_conn = Connection::open(path)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            sync_conn: Arc::new(std::sync::Mutex::new(sync_conn)),
            cipher: Arc::new(Cipher::new(master_key)),
        })
    }

    fn get_inner(
        conn: &Connection,
        cipher: &Cipher,
        scope: Scope,
        slug: Option<&str>,
        key: &str,
    ) -> Result<Option<String>> {
        let row: Option<(Vec<u8>, Vec<u8>)> = conn
            .query_row(
                "SELECT nonce, ciphertext FROM secrets
                 WHERE scope = ?1 AND COALESCE(slug, '') = COALESCE(?2, '') AND key = ?3",
                params![scope.as_str(), slug, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((nonce, ciphertext)) => {
                let aad_new = aad_for(scope.as_str(), slug, key);
                let aad_legacy = aad_for_legacy(scope.as_str(), slug, key);
                let plaintext =
                    cipher.open_with_aads(&nonce, &ciphertext, &[&aad_new, &aad_legacy])?;
                let s =
                    String::from_utf8(plaintext).context("decrypted secret is not valid UTF-8")?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }
}

#[async_trait]
impl SecretStore for SqliteSecretStore {
    /// List all secrets, values masked. Ordered by scope, slug, key.
    async fn list(&self) -> Result<Vec<SecretRecord>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, scope, slug, key, updated_at, updated_by
             FROM secrets ORDER BY scope, slug, key",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(SecretRecord {
                    id: r.get(0)?,
                    scope: r.get(1)?,
                    slug: r.get(2)?,
                    key: r.get(3)?,
                    updated_at: r.get(4)?,
                    updated_by: r.get(5)?,
                    masked_value: "••••••••".to_string(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Insert or update (upsert) a secret. Returns the new row id.
    async fn put(
        &self,
        scope: Scope,
        slug: Option<&str>,
        key: &str,
        plaintext: &str,
        updated_by: Option<i64>,
    ) -> Result<i64> {
        let aad = aad_for(scope.as_str(), slug, key);
        let (nonce, ciphertext) = self.cipher.seal(plaintext.as_bytes(), &aad)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO secrets (scope, slug, key, nonce, ciphertext, updated_at, updated_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(scope, COALESCE(slug, ''), key) DO UPDATE SET
                 nonce = excluded.nonce,
                 ciphertext = excluded.ciphertext,
                 updated_at = excluded.updated_at,
                 updated_by = excluded.updated_by",
            params![
                scope.as_str(),
                slug,
                key,
                nonce,
                ciphertext,
                now,
                updated_by
            ],
        )?;
        // Best-effort audit row.
        let _ = conn.execute(
            "INSERT INTO secrets_audit (scope, slug, key, action, user_id)
             VALUES (?1, ?2, ?3, 'put', ?4)",
            params![scope.as_str(), slug, key, updated_by],
        );
        Ok(conn.last_insert_rowid())
    }

    /// Read & decrypt a secret. Returns `None` if not present.
    ///
    /// The async connection Mutex is held only for the row read and released
    /// before the AES-GCM decrypt, so concurrent lookups do not serialize
    /// behind one another's crypto work.
    async fn get(&self, scope: Scope, slug: Option<&str>, key: &str) -> Result<Option<String>> {
        let row: Option<(Vec<u8>, Vec<u8>)> = {
            let conn = self.conn.lock().await;
            conn.query_row(
                "SELECT nonce, ciphertext FROM secrets
                 WHERE scope = ?1 AND COALESCE(slug, '') = COALESCE(?2, '') AND key = ?3",
                params![scope.as_str(), slug, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        };
        match row {
            Some((nonce, ciphertext)) => {
                let aad_new = aad_for(scope.as_str(), slug, key);
                let aad_legacy = aad_for_legacy(scope.as_str(), slug, key);
                let plaintext =
                    self.cipher
                        .open_with_aads(&nonce, &ciphertext, &[&aad_new, &aad_legacy])?;
                let s =
                    String::from_utf8(plaintext).context("decrypted secret is not valid UTF-8")?;
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    /// Synchronous variant of `get` — usable from non-async code
    /// (scheduler/poller). Uses a separate connection so it does not
    /// contend with the async writers.
    fn get_blocking(&self, scope: Scope, slug: Option<&str>, key: &str) -> Result<Option<String>> {
        let conn = self
            .sync_conn
            .lock()
            .map_err(|e| anyhow::anyhow!("sync_conn poisoned: {e}"))?;
        Self::get_inner(&conn, &self.cipher, scope, slug, key)
    }

    /// Decrypt a secret BY ID and write an audit row.
    async fn reveal(&self, id: i64, user_id: Option<i64>) -> Result<Option<String>> {
        // Read the row under the lock, then release it before the AES-GCM
        // decrypt so the connection Mutex does not serialize crypto work.
        let row: Option<(String, Option<String>, String, Vec<u8>, Vec<u8>)> = {
            let conn = self.conn.lock().await;
            conn.query_row(
                "SELECT scope, slug, key, nonce, ciphertext FROM secrets WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
        };
        match row {
            Some((scope, slug, key, nonce, ct)) => {
                let aad_new = aad_for(&scope, slug.as_deref(), &key);
                let aad_legacy = aad_for_legacy(&scope, slug.as_deref(), &key);
                let pt = self
                    .cipher
                    .open_with_aads(&nonce, &ct, &[&aad_new, &aad_legacy])?;
                let s = String::from_utf8(pt).context("not utf-8")?;
                // Re-acquire the lock only for the best-effort audit insert.
                {
                    let conn = self.conn.lock().await;
                    let _ = conn.execute(
                        "INSERT INTO secrets_audit (scope, slug, key, action, user_id)
                         VALUES (?1, ?2, ?3, 'reveal', ?4)",
                        params![scope, slug, key, user_id],
                    );
                }
                Ok(Some(s))
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, id: i64, user_id: Option<i64>) -> Result<bool> {
        let conn = self.conn.lock().await;
        // Capture identity for audit before delete.
        let row: Option<(String, Option<String>, String)> = conn
            .query_row(
                "SELECT scope, slug, key FROM secrets WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let n = conn.execute("DELETE FROM secrets WHERE id = ?1", params![id])?;
        if n > 0 {
            if let Some((scope, slug, key)) = row {
                let _ = conn.execute(
                    "INSERT INTO secrets_audit (scope, slug, key, action, user_id)
                     VALUES (?1, ?2, ?3, 'delete', ?4)",
                    params![scope, slug, key, user_id],
                );
            }
        }
        Ok(n > 0)
    }
}

fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS secrets (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            scope       TEXT NOT NULL CHECK (scope IN ('global','repo')),
            slug        TEXT,
            key         TEXT NOT NULL,
            nonce       BLOB NOT NULL,
            ciphertext  BLOB NOT NULL,
            updated_at  TEXT NOT NULL,
            updated_by  INTEGER
        );
        -- COALESCE(slug, '') so the unique index treats NULL slugs as identical.
        CREATE UNIQUE INDEX IF NOT EXISTS idx_secrets_uniq
            ON secrets(scope, COALESCE(slug, ''), key);
        CREATE TABLE IF NOT EXISTS secrets_audit (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            scope       TEXT NOT NULL,
            slug        TEXT,
            key         TEXT NOT NULL,
            action      TEXT NOT NULL,
            user_id     INTEGER,
            at          TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_audit_at ON secrets_audit(at DESC);",
    )?;
    Ok(())
}
