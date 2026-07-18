pub mod backend;
pub mod events;
pub mod issues;
pub mod licenses;
pub mod pulls;
pub mod schema;
pub mod search;
pub mod sync;
pub mod triage;
pub mod usage;

pub use search::{sanitize_query, SearchHit};
pub use usage::{UsageByKind, UsageCounts};

pub use backend::DatabaseBackend;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use crate::config::Config;

/// Parse labels JSON from DB, logging a warning on corrupt data.
pub fn parse_labels_json(json_str: &str) -> Vec<String> {
    serde_json::from_str(json_str).unwrap_or_else(|_| {
        tracing::warn!("Corrupt labels JSON in DB, defaulting to empty");
        Vec::new()
    })
}

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(config: &Config) -> Result<Self> {
        let db_path = config.wshm_dir.join("state.db");
        Self::open_path(&db_path)
    }

    pub fn open_path(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database: {}", path.display()))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )?;

        let db = Self {
            conn: Mutex::new(conn),
        };

        db.migrate()?;
        Ok(db)
    }

    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        let db = Self {
            conn: Mutex::new(conn),
        };

        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        schema::run_migrations(&conn)
    }

    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&conn)
    }

    /// Wipe every triage result and PR analysis. Used by the `revert` flow.
    /// Lives here (rather than in the revert pipeline) so the same operation
    /// is available to any `DatabaseBackend` impl.
    pub fn clear_triage_and_analyses(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM triage_results", [])?;
            conn.execute("DELETE FROM pr_analyses", [])?;
            Ok(())
        })
    }
}

/// Open a database backend based on the config.
///
/// OSS only ships the SQLite backend. The PostgreSQL backend is provided
/// by wshm-pro for self-hosted enterprise deployments. Setting
/// `database.provider = "postgresql"` in OSS is rejected with a clear
/// error.
pub fn open_backend(config: &Config) -> Result<Box<dyn DatabaseBackend>> {
    let provider = config
        .database
        .as_ref()
        .map(|d| d.provider.as_str())
        .unwrap_or("sqlite");

    match provider {
        "postgresql" | "postgres" => {
            anyhow::bail!(
                "PostgreSQL backend is a wshm-pro feature. \
                 Use the SQLite backend in OSS, or upgrade to wshm-pro."
            );
        }
        _ => {
            let db = Database::open(config)?;
            Ok(Box::new(db))
        }
    }
}
