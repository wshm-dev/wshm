use anyhow::Result;
use async_trait::async_trait;

use crate::config::DatabaseExportConfig;
use crate::export::{ExportEvent, ExportSink};

use super::validate_identifier;

/// PostgreSQL sink. Auto-creates the events table on first use.
pub struct PostgresSink {
    pool: sqlx::PgPool,
    table: String,
    initialized: tokio::sync::OnceCell<()>,
}

impl PostgresSink {
    pub fn new(config: &DatabaseExportConfig) -> Result<Self> {
        let uri = config.uri.as_deref().unwrap_or("postgres://localhost/wshm");
        let pool = sqlx::pool::PoolOptions::new()
            .max_connections(2)
            .connect_lazy(uri)?;
        let table = config
            .index
            .clone()
            .unwrap_or_else(|| "wshm_events".to_string());

        validate_identifier(&table)?;

        Ok(Self {
            pool,
            table,
            initialized: tokio::sync::OnceCell::new(),
        })
    }

    async fn ensure_table(&self) -> Result<()> {
        self.initialized
            .get_or_try_init(|| async {
                let sql = format!(
                    "CREATE TABLE IF NOT EXISTS {} (
                        id BIGSERIAL PRIMARY KEY,
                        kind TEXT NOT NULL,
                        repo TEXT NOT NULL,
                        timestamp TIMESTAMPTZ NOT NULL,
                        data JSONB NOT NULL
                    )",
                    self.table
                );
                sqlx::query(&sql).execute(&self.pool).await?;
                Ok::<(), anyhow::Error>(())
            })
            .await?;
        Ok(())
    }
}

#[async_trait]
impl ExportSink for PostgresSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        self.ensure_table().await?;

        let sql = format!(
            "INSERT INTO {} (kind, repo, timestamp, data) VALUES ($1, $2, $3, $4)",
            self.table
        );

        sqlx::query(&sql)
            .bind(event.kind.as_str())
            .bind(&event.repo)
            .bind(event.timestamp)
            .bind(&event.data)
            .execute(&self.pool)
            .await?;

        tracing::debug!("PostgreSQL: inserted into {}", self.table);
        Ok(())
    }

    fn name(&self) -> &str {
        "postgresql"
    }
}
