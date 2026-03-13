use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;

use crate::config::StorageConfig;
use crate::export::{ExportEvent, ExportSink};

/// S3-compatible storage sink. Writes JSON-lines per event, partitioned by date.
pub struct S3Sink {
    bucket: String,
    prefix: String,
    region: String,
    client: aws_sdk_s3::Client,
}

impl S3Sink {
    pub async fn new(config: &StorageConfig) -> Result<Self> {
        let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(
                config
                    .region
                    .clone()
                    .unwrap_or_else(|| "us-east-1".to_string()),
            ))
            .load()
            .await;

        let client = aws_sdk_s3::Client::new(&sdk_config);

        Ok(Self {
            bucket: config.bucket.clone().unwrap_or_default(),
            prefix: config.prefix.clone().unwrap_or_default(),
            region: config
                .region
                .clone()
                .unwrap_or_else(|| "us-east-1".to_string()),
            client,
        })
    }
}

#[async_trait]
impl ExportSink for S3Sink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        let date = Utc::now().format("%Y/%m/%d");
        let key = format!(
            "{}{}/{}-{}.json",
            self.prefix,
            date,
            event.kind.as_str(),
            event.timestamp.timestamp_millis()
        );

        let body = serde_json::to_vec(event)?;

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(body.into())
            .content_type("application/json")
            .send()
            .await?;

        tracing::debug!("S3: wrote s3://{}/{}", self.bucket, key);
        Ok(())
    }

    fn name(&self) -> &str {
        "s3"
    }
}
