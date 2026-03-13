use anyhow::Result;
use async_trait::async_trait;
use mysql_async::prelude::*;

use crate::config::DatabaseExportConfig;
use crate::export::{ExportEvent, ExportSink};

/// MySQL/MariaDB sink. Auto-creates the events table on first use.
pub struct MysqlSink {
    pool: mysql_async::Pool,
    table: String,
    initialized: tokio::sync::OnceCell<()>,
}

impl MysqlSink {
    pub fn new(config: &DatabaseExportConfig) -> Result<Self> {
        let uri = config
            .uri
            .as_deref()
            .unwrap_or("mysql://root@localhost/wshm");
        let pool = mysql_async::Pool::new(uri);

        Ok(Self {
            pool,
            table: config
                .index
                .clone()
                .unwrap_or_else(|| "wshm_events".to_string()),
            initialized: tokio::sync::OnceCell::new(),
        })
    }

    async fn ensure_table(&self) -> Result<()> {
        self.initialized
            .get_or_try_init(|| async {
                let mut conn = self.pool.get_conn().await?;
                let sql = format!(
                    "CREATE TABLE IF NOT EXISTS {} (
                        id BIGINT AUTO_INCREMENT PRIMARY KEY,
                        kind VARCHAR(64) NOT NULL,
                        repo VARCHAR(255) NOT NULL,
                        timestamp DATETIME(3) NOT NULL,
                        data JSON NOT NULL,
                        INDEX idx_kind (kind),
                        INDEX idx_repo (repo),
                        INDEX idx_timestamp (timestamp)
                    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
                    self.table
                );
                conn.query_drop(sql).await?;
                Ok::<(), anyhow::Error>(())
            })
            .await?;
        Ok(())
    }
}

#[async_trait]
impl ExportSink for MysqlSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        self.ensure_table().await?;

        let mut conn = self.pool.get_conn().await?;
        let sql = format!(
            "INSERT INTO {} (kind, repo, timestamp, data) VALUES (:kind, :repo, :ts, :data)",
            self.table
        );

        let data_json = serde_json::to_string(&event.data)?;
        let ts = event.timestamp.format("%Y-%m-%d %H:%M:%S%.3f").to_string();

        conn.exec_drop(
            sql,
            params! {
                "kind" => event.kind.as_str(),
                "repo" => &event.repo,
                "ts" => ts,
                "data" => data_json,
            },
        )
        .await?;

        tracing::debug!("MySQL: inserted into {}", self.table);
        Ok(())
    }

    fn name(&self) -> &str {
        "mysql"
    }
}

impl Drop for MysqlSink {
    fn drop(&mut self) {
        // Pool cleanup is async, best-effort
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let _ = pool.disconnect().await;
        });
    }
}
