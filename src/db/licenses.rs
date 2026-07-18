//! License JWT persistence in SQLite.
//!
//! Single-row table (`licenses` with `CHECK (id = 1)`) so the active
//! license survives pod restarts and the historical class of bugs where
//! `~/.wshm/license.jwt` would vanish from the PVC after an interrupted
//! `fs::write`. The web UI's POST `/api/v1/license/activate` and the CLI
//! `wshm login --license` both write through this module; readers go
//! through `crate::license::resolve_*` which checks the DB first.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// One row of the `licenses` table (always `id = 1`).
#[derive(Debug, Clone)]
pub struct StoredLicense {
    pub jwt: String,
    pub license_key: Option<String>,
    pub plan: Option<String>,
    pub activated_at: String,
    pub activated_by: Option<String>,
    pub expires_at: Option<String>,
    pub last_validated_at: Option<String>,
}

/// Optional sink that mirrors every `store()` call to an alternate
/// backend (e.g. Postgres in wshm-pro stateless mode). Registered once
/// at process startup; missing hook = no-op (OSS default).
pub type LicenseStoreHook = fn(
    jwt: &str,
    license_key: Option<&str>,
    plan: Option<&str>,
    activated_by: Option<&str>,
    expires_at: Option<&str>,
);

static LICENSE_STORE_HOOK: std::sync::OnceLock<LicenseStoreHook> = std::sync::OnceLock::new();

pub fn set_license_store_hook(f: LicenseStoreHook) {
    let _ = LICENSE_STORE_HOOK.set(f);
}

/// Upsert the active license. Subsequent activations overwrite the row
/// in place (single-row table) so callers don't have to delete first.
pub fn store(
    conn: &Connection,
    jwt: &str,
    license_key: Option<&str>,
    plan: Option<&str>,
    activated_by: Option<&str>,
    expires_at: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO licenses (id, jwt, license_key, plan, activated_at, activated_by, expires_at, last_validated_at)
         VALUES (1, ?1, ?2, ?3, datetime('now'), ?4, ?5, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET
            jwt               = excluded.jwt,
            license_key       = COALESCE(excluded.license_key, license_key),
            plan              = COALESCE(excluded.plan, plan),
            activated_at      = excluded.activated_at,
            activated_by      = COALESCE(excluded.activated_by, activated_by),
            expires_at        = COALESCE(excluded.expires_at, expires_at),
            last_validated_at = excluded.last_validated_at",
        params![jwt, license_key, plan, activated_by, expires_at],
    )
    .context("Failed to upsert license row")?;
    if let Some(hook) = LICENSE_STORE_HOOK.get() {
        hook(jwt, license_key, plan, activated_by, expires_at);
    }
    Ok(())
}

pub fn load(conn: &Connection) -> Result<Option<StoredLicense>> {
    conn.query_row(
        "SELECT jwt, license_key, plan, activated_at, activated_by, expires_at, last_validated_at
         FROM licenses WHERE id = 1",
        [],
        |row| {
            Ok(StoredLicense {
                jwt: row.get(0)?,
                license_key: row.get(1)?,
                plan: row.get(2)?,
                activated_at: row.get(3)?,
                activated_by: row.get(4)?,
                expires_at: row.get(5)?,
                last_validated_at: row.get(6)?,
            })
        },
    )
    .optional()
    .context("Failed to load license row")
}

pub fn load_jwt(conn: &Connection) -> Result<Option<String>> {
    Ok(load(conn)?.map(|l| l.jwt))
}

pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM licenses WHERE id = 1", [])
        .context("Failed to clear license row")?;
    Ok(())
}

/// Open `state.db` at the given path, ensuring the licenses table
/// exists. Used by `crate::license::*` which doesn't have access to the
/// long-lived `Database` handle (resolve runs at startup before the
/// daemon's DB pool is built).
pub fn open_state_db(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let conn = Connection::open(path)
        .with_context(|| format!("Failed to open state.db at {}", path.display()))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS licenses (
            id                INTEGER PRIMARY KEY CHECK (id = 1),
            jwt               TEXT NOT NULL,
            license_key       TEXT,
            plan              TEXT,
            activated_at      TEXT NOT NULL,
            activated_by      TEXT,
            expires_at        TEXT,
            last_validated_at TEXT
        );",
    )?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE licenses (
                id                INTEGER PRIMARY KEY CHECK (id = 1),
                jwt               TEXT NOT NULL,
                license_key       TEXT,
                plan              TEXT,
                activated_at      TEXT NOT NULL,
                activated_by      TEXT,
                expires_at        TEXT,
                last_validated_at TEXT
            );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn store_then_load_roundtrip() {
        let conn = fresh_conn();
        store(
            &conn,
            "jwt-a",
            Some("KEY-1"),
            Some("pro"),
            Some("alice"),
            None,
        )
        .unwrap();
        let got = load(&conn).unwrap().unwrap();
        assert_eq!(got.jwt, "jwt-a");
        assert_eq!(got.license_key.as_deref(), Some("KEY-1"));
        assert_eq!(got.plan.as_deref(), Some("pro"));
        assert_eq!(got.activated_by.as_deref(), Some("alice"));
    }

    #[test]
    fn store_upserts_in_place() {
        let conn = fresh_conn();
        store(&conn, "jwt-a", Some("KEY-1"), Some("pro"), None, None).unwrap();
        store(&conn, "jwt-b", None, None, None, None).unwrap();
        let got = load(&conn).unwrap().unwrap();
        assert_eq!(got.jwt, "jwt-b");
        // COALESCE preserves the previous license_key + plan when new
        // call passes None — so refresh-only updates don't wipe metadata.
        assert_eq!(got.license_key.as_deref(), Some("KEY-1"));
        assert_eq!(got.plan.as_deref(), Some("pro"));
    }

    #[test]
    fn clear_removes_row() {
        let conn = fresh_conn();
        store(&conn, "jwt-a", None, None, None, None).unwrap();
        clear(&conn).unwrap();
        assert!(load(&conn).unwrap().is_none());
    }

    #[test]
    fn load_returns_none_when_empty() {
        let conn = fresh_conn();
        assert!(load(&conn).unwrap().is_none());
    }

    #[test]
    fn check_constraint_blocks_second_row() {
        let conn = fresh_conn();
        store(&conn, "jwt-a", None, None, None, None).unwrap();
        let err = conn.execute(
            "INSERT INTO licenses (id, jwt, activated_at) VALUES (2, 'x', datetime('now'))",
            [],
        );
        assert!(err.is_err(), "CHECK (id = 1) must reject id=2");
    }
}
