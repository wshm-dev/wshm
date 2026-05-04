//! RBAC + local accounts.
//!
//! Owns a daemon-level SQLite database (separate from the per-repo DBs) at
//! `$WSHM_HOME/users.db` storing user identities (local + SSO-upserted),
//! password hashes (argon2), and roles. Exposed to the web layer via
//! [`UserStore`] so handlers can authenticate, authorize, and CRUD users.
//!
//! When the daemon is started without a UserStore wired into the web state,
//! the legacy single `[web].username/password` Basic Auth flow is used and
//! there is no concept of multiple users or roles.

use anyhow::{anyhow, Context, Result};
use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
    Viewer,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
            Role::Viewer => "viewer",
        }
    }
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "admin" => Ok(Role::Admin),
            "member" => Ok(Role::Member),
            "viewer" => Ok(Role::Viewer),
            other => Err(anyhow!("invalid role: {other}")),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub username: Option<String>,
    pub role: Role,
    pub sso_provider: Option<String>,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

#[derive(Clone)]
pub struct UserStore {
    conn: Arc<Mutex<Connection>>,
}

impl UserStore {
    /// Open or create the users database at `path` and run schema migration.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening users db at {}", path.display()))?;
        run_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn count(&self) -> Result<i64> {
        let conn = self.conn.lock().await;
        Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?)
    }

    pub async fn list(&self) -> Result<Vec<User>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, email, username, role, sso_provider, created_at, last_login_at
             FROM users ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map([], row_to_user)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<User>> {
        let conn = self.conn.lock().await;
        Ok(conn
            .query_row(
                "SELECT id, email, username, role, sso_provider, created_at, last_login_at
                 FROM users WHERE id = ?1",
                params![id],
                row_to_user,
            )
            .optional()?)
    }

    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>> {
        let conn = self.conn.lock().await;
        Ok(conn
            .query_row(
                "SELECT id, email, username, role, sso_provider, created_at, last_login_at
                 FROM users WHERE email = ?1",
                params![email],
                row_to_user,
            )
            .optional()?)
    }

    /// Look up by either email or username (login form accepts both).
    pub async fn find_by_login(&self, login: &str) -> Result<Option<(User, Option<String>)>> {
        let conn = self.conn.lock().await;
        Ok(conn
            .query_row(
                "SELECT id, email, username, role, sso_provider, created_at, last_login_at,
                        password_hash
                 FROM users WHERE email = ?1 OR username = ?1",
                params![login],
                |row| {
                    let user = row_to_user(row)?;
                    let hash: Option<String> = row.get(7)?;
                    Ok((user, hash))
                },
            )
            .optional()?)
    }

    /// Create a new local-credential user. Hashes the password with argon2.
    /// Returns the new user's id.
    pub async fn create_local(
        &self,
        email: &str,
        username: Option<&str>,
        password: &str,
        role: Role,
    ) -> Result<i64> {
        let hash = hash_password(password)?;
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO users (email, username, password_hash, role, sso_provider,
                                created_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params![email, username, hash, role.as_str(), now],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Insert (or update last_login_at on) an SSO-authenticated user. Default
    /// role is `member`; existing rows keep their assigned role. If the email
    /// matches `WSHM_ADMIN_EMAIL` and the row is being created (not updated),
    /// the user is promoted to `admin` so the bootstrap operator gets in.
    pub async fn upsert_sso(
        &self,
        email: &str,
        username: Option<&str>,
        provider: &str,
    ) -> Result<User> {
        let now = Utc::now().to_rfc3339();
        let bootstrap_admin_role = std::env::var("WSHM_ADMIN_EMAIL")
            .ok()
            .filter(|admin_email| admin_email.eq_ignore_ascii_case(email))
            .map(|_| "admin")
            .unwrap_or("member");
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO users (email, username, password_hash, role, sso_provider,
                                created_at, last_login_at)
             VALUES (?1, ?2, NULL, ?5, ?3, ?4, ?4)
             ON CONFLICT(email) DO UPDATE SET
                 username = COALESCE(excluded.username, users.username),
                 sso_provider = excluded.sso_provider,
                 last_login_at = excluded.last_login_at",
            params![email, username, provider, now, bootstrap_admin_role],
        )?;
        let user = conn
            .query_row(
                "SELECT id, email, username, role, sso_provider, created_at, last_login_at
                 FROM users WHERE email = ?1",
                params![email],
                row_to_user,
            )?;
        Ok(user)
    }

    pub async fn update_role(&self, id: i64, role: Role) -> Result<()> {
        let conn = self.conn.lock().await;
        let n = conn.execute(
            "UPDATE users SET role = ?1 WHERE id = ?2",
            params![role.as_str(), id],
        )?;
        if n == 0 {
            return Err(anyhow!("user {id} not found"));
        }
        Ok(())
    }

    pub async fn update_password(&self, id: i64, password: &str) -> Result<()> {
        let hash = hash_password(password)?;
        let conn = self.conn.lock().await;
        let n = conn.execute(
            "UPDATE users SET password_hash = ?1 WHERE id = ?2",
            params![hash, id],
        )?;
        if n == 0 {
            return Err(anyhow!("user {id} not found"));
        }
        Ok(())
    }

    pub async fn delete(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().await;
        let n = conn.execute("DELETE FROM users WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(anyhow!("user {id} not found"));
        }
        Ok(())
    }

    pub async fn touch_login(&self, id: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE users SET last_login_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        Ok(())
    }
}

fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    let role_str: String = row.get(3)?;
    let role = Role::from_str(&role_str).unwrap_or(Role::Viewer);
    Ok(User {
        id: row.get(0)?,
        email: row.get(1)?,
        username: row.get(2)?,
        role,
        sso_provider: row.get(4)?,
        created_at: row.get(5)?,
        last_login_at: row.get(6)?,
    })
}

fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS users (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            email           TEXT NOT NULL UNIQUE,
            username        TEXT UNIQUE,
            password_hash   TEXT,
            role            TEXT NOT NULL DEFAULT 'member',
            sso_provider    TEXT,
            created_at      TEXT NOT NULL,
            last_login_at   TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);",
    )?;
    Ok(())
}

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("hash_password: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// First-boot admin seed. If the `users` table is empty, create an admin
/// row from `WSHM_ADMIN_EMAIL` + `WSHM_ADMIN_PASSWORD` env vars. If only
/// `WSHM_ADMIN_EMAIL` is set, generate a random password and log it once
/// to stdout so the operator can capture it from `kubectl logs`.
pub async fn seed_admin_if_empty(store: &UserStore) -> Result<()> {
    if store.count().await? > 0 {
        return Ok(());
    }
    let email = std::env::var("WSHM_ADMIN_EMAIL").ok();
    let email = match email {
        Some(e) if !e.trim().is_empty() => e,
        _ => {
            tracing::warn!(
                "users table is empty and WSHM_ADMIN_EMAIL is not set — \
                 nobody can log in via local accounts. Set the env var or \
                 sign in via SSO (oauth2-proxy) to bootstrap."
            );
            return Ok(());
        }
    };
    let password = match std::env::var("WSHM_ADMIN_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            let p = generate_password();
            tracing::warn!(
                target: "wshm_core::auth",
                "Seeded admin {email} with generated password: {p} \
                 (rotate via Settings → Users → change password)"
            );
            p
        }
    };
    store
        .create_local(&email, None, &password, Role::Admin)
        .await?;
    tracing::info!("Seeded initial admin user: {email}");
    Ok(())
}

fn generate_password() -> String {
    use rand::distributions::{Alphanumeric, DistString};
    Alphanumeric.sample_string(&mut rand::thread_rng(), 24)
}
