pub mod backend;
pub mod events;
pub mod issues;
pub mod postgres;
pub mod pulls;
pub mod schema;
pub mod sync;
pub mod triage;

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

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;

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
}

/// Open a database backend based on the config.
///
/// - `"sqlite"` (default): opens the local `.wshm/state.db` SQLite database.
/// - `"postgresql"`: connects to the configured PostgreSQL instance (requires `database-postgres` feature).
///
/// Returns the backend as a boxed trait object for uniform usage.
pub fn open_backend(config: &Config) -> Result<Box<dyn DatabaseBackend>> {
    let provider = config
        .database
        .as_ref()
        .map(|d| d.provider.as_str())
        .unwrap_or("sqlite");

    match provider {
        #[cfg(feature = "database-postgres")]
        "postgresql" | "postgres" => {
            let rt = tokio::runtime::Handle::current();
            let pg = rt.block_on(postgres::PostgresDb::connect(config))?;
            Ok(Box::new(pg))
        }
        #[cfg(not(feature = "database-postgres"))]
        "postgresql" | "postgres" => {
            anyhow::bail!(
                "PostgreSQL backend requested but the 'database-postgres' feature is not enabled. \
                 Rebuild with: cargo build --features database-postgres"
            );
        }
        _ => {
            let db = Database::open(config)?;
            Ok(Box::new(db))
        }
    }
}
