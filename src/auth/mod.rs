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
use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Abstract store interface so wshm-pro can plug in a Postgres-backed
/// implementation for stateless K8s deploys (where the per-pod users.db
/// would be wiped on restart). OSS keeps using the SQLite `UserStore`
/// below; Pro provides its own type that implements this trait.
#[async_trait]
pub trait UserStoreBackend: Send + Sync {
    async fn count(&self) -> Result<i64>;
    async fn list(&self) -> Result<Vec<User>>;
    async fn find_by_id(&self, id: i64) -> Result<Option<User>>;
    async fn find_by_email(&self, email: &str) -> Result<Option<User>>;
    async fn find_by_login(&self, login: &str) -> Result<Option<(User, Option<String>)>>;
    async fn create_local(
        &self,
        email: &str,
        username: Option<&str>,
        password: &str,
        role: Role,
    ) -> Result<i64>;
    async fn upsert_sso(&self, email: &str, username: Option<&str>, provider: &str)
        -> Result<User>;
    async fn update_role(&self, id: i64, role: Role) -> Result<()>;
    async fn update_password(&self, id: i64, password: &str) -> Result<()>;
    async fn delete(&self, id: i64) -> Result<()>;
    async fn touch_login(&self, id: i64) -> Result<()>;
}

#[async_trait]
impl UserStoreBackend for UserStore {
    async fn count(&self) -> Result<i64> {
        UserStore::count(self).await
    }
    async fn list(&self) -> Result<Vec<User>> {
        UserStore::list(self).await
    }
    async fn find_by_id(&self, id: i64) -> Result<Option<User>> {
        UserStore::find_by_id(self, id).await
    }
    async fn find_by_email(&self, email: &str) -> Result<Option<User>> {
        UserStore::find_by_email(self, email).await
    }
    async fn find_by_login(&self, login: &str) -> Result<Option<(User, Option<String>)>> {
        UserStore::find_by_login(self, login).await
    }
    async fn create_local(
        &self,
        email: &str,
        username: Option<&str>,
        password: &str,
        role: Role,
    ) -> Result<i64> {
        UserStore::create_local(self, email, username, password, role).await
    }
    async fn upsert_sso(
        &self,
        email: &str,
        username: Option<&str>,
        provider: &str,
    ) -> Result<User> {
        UserStore::upsert_sso(self, email, username, provider).await
    }
    async fn update_role(&self, id: i64, role: Role) -> Result<()> {
        UserStore::update_role(self, id, role).await
    }
    async fn update_password(&self, id: i64, password: &str) -> Result<()> {
        UserStore::update_password(self, id, password).await
    }
    async fn delete(&self, id: i64) -> Result<()> {
        UserStore::delete(self, id).await
    }
    async fn touch_login(&self, id: i64) -> Result<()> {
        UserStore::touch_login(self, id).await
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Member,
    Operator,
    Admin,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Operator => "operator",
            Role::Member => "member",
            Role::Viewer => "viewer",
        }
    }
    #[allow(clippy::should_implement_trait)] // not infallible enough for FromStr trait shape
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "admin" => Ok(Role::Admin),
            "operator" => Ok(Role::Operator),
            "member" => Ok(Role::Member),
            "viewer" => Ok(Role::Viewer),
            other => Err(anyhow!("invalid role: {other}")),
        }
    }
    /// True if this role can perform actions requiring at least `min`.
    /// Variants are ordered Viewer < Member < Operator < Admin.
    pub fn has_at_least(self, min: Role) -> bool {
        self >= min
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
        // Re-creating a user via the admin UI is a deliberate decision to
        // re-grant access, so clear the SSO-promotion tombstone.
        conn.execute(
            "DELETE FROM deleted_emails WHERE email = ?1",
            params![email],
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
        // M-3: bootstrap admin promotion is suppressed for tombstoned
        // emails. An admin who deletes a user means it; SSO re-login
        // mustn't silently restore privileges via WSHM_ADMIN_EMAIL.
        let tombstoned = self.is_email_tombstoned(email).await.unwrap_or(false);
        let bootstrap_admin_role = if tombstoned {
            "member"
        } else {
            std::env::var("WSHM_ADMIN_EMAIL")
                .ok()
                .filter(|admin_email| admin_email.eq_ignore_ascii_case(email))
                .map(|_| "admin")
                .unwrap_or("member")
        };
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
        let user = conn.query_row(
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
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().await;
        // Tombstone: keep the email in `deleted_emails` so a future
        // SSO upsert with the same address cannot silently re-claim
        // admin via the WSHM_ADMIN_EMAIL bootstrap branch (the audit's
        // M-3 finding). Wrap in a tx so the email never disappears
        // from the DB without being recorded.
        let email: Option<String> = conn
            .query_row("SELECT email FROM users WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .optional()?;
        let n = conn.execute("DELETE FROM users WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(anyhow!("user {id} not found"));
        }
        if let Some(e) = email {
            conn.execute(
                "INSERT INTO deleted_emails (email, deleted_at) VALUES (?1, ?2)
                 ON CONFLICT(email) DO UPDATE SET deleted_at = excluded.deleted_at",
                params![e, now],
            )?;
        }
        Ok(())
    }

    /// True if this email was previously deleted by an admin. Used to
    /// stop the WSHM_ADMIN_EMAIL bootstrap from re-promoting a user
    /// that was demoted-by-deletion.
    async fn is_email_tombstoned(&self, email: &str) -> Result<bool> {
        let conn = self.conn.lock().await;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM deleted_emails WHERE email = ?1",
            params![email],
            |r| r.get(0),
        )?;
        Ok(n > 0)
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
        CREATE INDEX IF NOT EXISTS idx_users_email ON users(email);
        -- Tombstone for previously-deleted users. Keeps SSO re-login
        -- from silently re-promoting a demoted email via the
        -- WSHM_ADMIN_EMAIL bootstrap branch. Cleared by an explicit
        -- admin who re-creates the user, see UserStore::create_local.
        CREATE TABLE IF NOT EXISTS deleted_emails (
            email      TEXT PRIMARY KEY,
            deleted_at TEXT NOT NULL
        );",
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

/// A valid argon2 PHC hash of a fixed throwaway password, computed once per
/// process. Login paths verify a supplied password against this when the
/// looked-up user (or its password hash) is missing, so a non-existent
/// account pays the same argon2 cost as a real wrong password — closing the
/// username-enumeration timing oracle.
pub fn dummy_password_hash() -> &'static str {
    use std::sync::OnceLock;
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        hash_password("wshm-dummy-password-for-constant-time-verification")
            .expect("hashing a fixed dummy password cannot fail")
    })
}

/// First-boot admin seed. If the `users` table is empty, create a local
/// admin account so the operator can log in.
///
/// Identifier resolution order:
/// 1. `WSHM_ADMIN_USER` — preferred, plain username (e.g. "admin").
/// 2. `WSHM_ADMIN_EMAIL` — legacy, also accepted for backwards compat.
/// 3. Default to `"admin"` so a fresh install always has a usable account.
///
/// Password resolution: `WSHM_ADMIN_PASSWORD` if set, otherwise a random
/// 24-char password is generated and logged once at WARN level.
pub async fn seed_admin_if_empty(store: &dyn UserStoreBackend) -> Result<()> {
    if store.count().await? > 0 {
        return Ok(());
    }
    let identifier = std::env::var("WSHM_ADMIN_USER")
        .ok()
        .or_else(|| std::env::var("WSHM_ADMIN_EMAIL").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "admin".to_string());
    let (password, generated) = match std::env::var("WSHM_ADMIN_PASSWORD") {
        Ok(p) if !p.is_empty() => (p, false),
        _ => (generate_password(), true),
    };
    // Fall back to the identifier itself for the email column (NOT NULL)
    // when the operator picked a plain username. find_by_login matches
    // on either email or username, so the login form still works.
    let username_opt = if identifier.contains('@') {
        None
    } else {
        Some(identifier.as_str())
    };
    store
        .create_local(&identifier, username_opt, &password, Role::Admin)
        .await?;
    if generated {
        // The full credential is written to stderr only — it must NOT
        // land in the in-memory LogBuffer (served by /api/v1/logs and
        // readable by any authenticated user, including viewers).
        // Use eprintln! to bypass the tracing layer entirely.
        eprintln!(
            "===== Seeded initial admin =====\n\
             user:     {identifier}\n\
             password: {password}\n\
             Rotate via Settings → Users (or set WSHM_ADMIN_USER + \
             WSHM_ADMIN_PASSWORD before first boot to pick your own)."
        );
        tracing::warn!(
            target: "wshm_core::auth",
            "Seeded initial admin user: {identifier} \
             (password printed to stderr — rotate via Settings → Users)"
        );
    } else {
        tracing::info!("Seeded initial admin user: {identifier}");
    }
    Ok(())
}

fn generate_password() -> String {
    use rand::distributions::{Alphanumeric, DistString};
    Alphanumeric.sample_string(&mut rand::thread_rng(), 24)
}
