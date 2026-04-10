use anyhow::Result;
use async_trait::async_trait;

use crate::config::DatabaseExportConfig;
use crate::export::{ExportEvent, ExportSink};

/// MongoDB sink.
pub struct MongoSink {
    collection: ::mongodb::Collection<serde_json::Value>,
}

impl MongoSink {
    pub async fn new(config: &DatabaseExportConfig) -> Result<Self> {
        let uri = config.uri.as_deref().unwrap_or("mongodb://localhost:27017");
        let client = ::mongodb::Client::with_uri_str(uri).await?;
        let db_name = config.database.as_deref().unwrap_or("wshm");
        let collection_name = config.index.as_deref().unwrap_or("events");

        let collection = client.database(db_name).collection(collection_name);

        Ok(Self { collection })
    }
}

#[async_trait]
impl ExportSink for MongoSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        let doc = serde_json::to_value(event)?;
        self.collection.insert_one(doc).await?;
        tracing::debug!("MongoDB: inserted event");
        Ok(())
    }

    fn name(&self) -> &str {
        "mongodb"
    }
}
