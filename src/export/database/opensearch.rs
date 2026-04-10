use anyhow::Result;
use async_trait::async_trait;

use crate::config::DatabaseExportConfig;
use crate::export::{ExportEvent, ExportSink};

/// OpenSearch sink.
pub struct OpenSearchSink {
    index: String,
    client: opensearch::OpenSearch,
}

impl OpenSearchSink {
    pub fn new(config: &DatabaseExportConfig) -> Result<Self> {
        let uri = config.uri.as_deref().unwrap_or("http://localhost:9200");
        let transport = opensearch::http::transport::Transport::single_node(uri)?;
        let client = opensearch::OpenSearch::new(transport);

        Ok(Self {
            index: config
                .index
                .clone()
                .unwrap_or_else(|| "wshm-events".to_string()),
            client,
        })
    }
}

#[async_trait]
impl ExportSink for OpenSearchSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        let body = serde_json::to_value(event)?;

        self.client
            .index(opensearch::IndexParts::Index(&self.index))
            .body(body)
            .send()
            .await?;

        tracing::debug!("OpenSearch: indexed to {}", self.index);
        Ok(())
    }

    fn name(&self) -> &str {
        "opensearch"
    }
}
