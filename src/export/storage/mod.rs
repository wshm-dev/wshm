use anyhow::Result;

use crate::config::StorageConfig;

use super::ExportSink;

#[cfg(feature = "export-s3")]
pub mod s3;

#[cfg(feature = "export-azure")]
pub mod azure;

#[cfg(feature = "export-gcs")]
pub mod gcs;

/// Build a storage sink from config. Returns None if the required feature is not enabled.
pub fn build_sink(config: &StorageConfig) -> Result<Option<Box<dyn ExportSink>>> {
    match config.provider.as_str() {
        #[cfg(feature = "export-s3")]
        "s3" => Ok(Some(Box::new(s3::S3Sink::new(config)?))),

        #[cfg(feature = "export-azure")]
        "azure" => Ok(Some(Box::new(azure::AzureSink::new(config)?))),

        #[cfg(feature = "export-gcs")]
        "gcs" => Ok(Some(Box::new(gcs::GcsSink::new(config)?))),

        provider => {
            tracing::warn!(
                "Storage provider '{provider}' is not available. \
                 Compile with the corresponding feature flag (e.g., --features export-s3)."
            );
            Ok(None)
        }
    }
}
