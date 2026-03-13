use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;

use crate::config::StorageConfig;
use crate::export::{ExportEvent, ExportSink};

/// Azure Blob Storage sink.
pub struct AzureSink {
    container: String,
    prefix: String,
    client: azure_storage_blobs::prelude::ContainerClient,
}

impl AzureSink {
    pub fn new(config: &StorageConfig) -> Result<Self> {
        let account =
            std::env::var("AZURE_STORAGE_ACCOUNT").unwrap_or_else(|_| "default".to_string());
        let key = std::env::var("AZURE_STORAGE_KEY").unwrap_or_default();
        let container = config
            .bucket
            .clone()
            .unwrap_or_else(|| "wshm-logs".to_string());

        let credential = azure_storage::StorageCredentials::access_key(&account, key);
        let client = azure_storage_blobs::prelude::BlobServiceClient::new(&account, credential)
            .container_client(&container);

        Ok(Self {
            container,
            prefix: config.prefix.clone().unwrap_or_default(),
            client,
        })
    }
}

#[async_trait]
impl ExportSink for AzureSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        let date = Utc::now().format("%Y/%m/%d");
        let blob_name = format!(
            "{}{}/{}-{}.json",
            self.prefix,
            date,
            event.kind.as_str(),
            event.timestamp.timestamp_millis()
        );

        let body = serde_json::to_vec(event)?;

        self.client
            .blob_client(&blob_name)
            .put_block_blob(body)
            .await?;

        tracing::debug!("Azure: wrote {}/{}", self.container, blob_name);
        Ok(())
    }

    fn name(&self) -> &str {
        "azure-blob"
    }
}
