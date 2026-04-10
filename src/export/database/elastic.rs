use anyhow::Result;
use async_trait::async_trait;

use crate::config::DatabaseExportConfig;
use crate::export::{ExportEvent, ExportSink};

/// Elasticsearch sink.
pub struct ElasticSink {
    index: String,
    client: elasticsearch::Elasticsearch,
}

impl ElasticSink {
    pub fn new(config: &DatabaseExportConfig) -> Result<Self> {
        let uri = config.uri.as_deref().unwrap_or("http://localhost:9200");
        let transport = elasticsearch::http::transport::Transport::single_node(uri)?;
        let client = elasticsearch::Elasticsearch::new(transport);

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
impl ExportSink for ElasticSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        let body = serde_json::to_value(event)?;

        self.client
            .index(elasticsearch::IndexParts::Index(&self.index))
            .body(body)
            .send()
            .await?;

        tracing::debug!("Elasticsearch: indexed to {}", self.index);
        Ok(())
    }

    fn name(&self) -> &str {
        "elasticsearch"
    }
}
