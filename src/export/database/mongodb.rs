use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::OnceCell;

use crate::config::DatabaseExportConfig;
use crate::export::{ExportEvent, ExportSink};

/// MongoDB sink. The client is created lazily on the first event so the sink
/// can be built from the synchronous export config path.
pub struct MongoSink {
    uri: String,
    db_name: String,
    collection_name: String,
    collection: OnceCell<::mongodb::Collection<serde_json::Value>>,
}

impl MongoSink {
    pub fn new(config: &DatabaseExportConfig) -> Result<Self> {
        Ok(Self {
            uri: config
                .uri
                .clone()
                .unwrap_or_else(|| "mongodb://localhost:27017".to_string()),
            db_name: config.database.clone().unwrap_or_else(|| "wshm".into()),
            collection_name: config.index.clone().unwrap_or_else(|| "events".into()),
            collection: OnceCell::new(),
        })
    }

    async fn collection(&self) -> Result<&::mongodb::Collection<serde_json::Value>> {
        self.collection
            .get_or_try_init(|| async {
                let client = ::mongodb::Client::with_uri_str(&self.uri).await?;
                Ok::<_, anyhow::Error>(
                    client
                        .database(&self.db_name)
                        .collection(&self.collection_name),
                )
            })
            .await
    }
}

#[async_trait]
impl ExportSink for MongoSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        let doc = serde_json::to_value(event)?;
        self.collection().await?.insert_one(doc).await?;
        tracing::debug!("MongoDB: inserted event");
        Ok(())
    }

    fn name(&self) -> &str {
        "mongodb"
    }
}
