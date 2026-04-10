use anyhow::Result;
use async_trait::async_trait;

use crate::config::StorageConfig;
use crate::export::{ExportEvent, ExportSink};

/// Google Cloud Storage sink.
pub struct GcsSink {
    bucket: String,
    prefix: String,
    client: google_cloud_storage::client::Client,
}

impl GcsSink {
    pub async fn new(config: &StorageConfig) -> Result<Self> {
        let client_config = google_cloud_storage::client::ClientConfig::default()
            .with_auth()
            .await?;
        let client = google_cloud_storage::client::Client::new(client_config);

        Ok(Self {
            bucket: config.bucket.clone().unwrap_or_default(),
            prefix: config.prefix.clone().unwrap_or_default(),
            client,
        })
    }
}

#[async_trait]
impl ExportSink for GcsSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        let object_name = super::event_object_path(&self.prefix, event);
        let body = serde_json::to_vec(event)?;

        use google_cloud_storage::http::objects::upload::{Media, UploadObjectRequest, UploadType};
        let upload_type = UploadType::Simple(Media::new(object_name.clone()));
        self.client
            .upload_object(
                &UploadObjectRequest {
                    bucket: self.bucket.clone(),
                    ..Default::default()
                },
                body,
                &upload_type,
            )
            .await?;

        tracing::debug!("GCS: wrote gs://{}/{}", self.bucket, object_name);
        Ok(())
    }

    fn name(&self) -> &str {
        "gcs"
    }
}
