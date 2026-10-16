//! Cloud storage export sink (S3 / S3-compatible, GCS, Azure Blob, local
//! directory), built on [`crate::storage::ObjectTarget`].

use anyhow::Result;
use async_trait::async_trait;

use crate::config::StorageConfig;
use crate::storage::{provider_available, ObjectTarget};

use super::{ExportEvent, ExportSink};

/// Date-partitioned object key for an event, relative to the configured
/// prefix: `YYYY/MM/DD/<Kind>-<unix millis>.json` (date of the event).
pub fn event_object_key(event: &ExportEvent) -> String {
    format!(
        "{}/{}-{}.json",
        event.timestamp.format("%Y/%m/%d"),
        event.kind.as_str(),
        event.timestamp.timestamp_millis()
    )
}

/// Writes one JSON object per event.
pub struct ObjectStorageSink {
    target: ObjectTarget,
}

impl ObjectStorageSink {
    pub fn new(target: ObjectTarget) -> Self {
        Self { target }
    }
}

#[async_trait]
impl ExportSink for ObjectStorageSink {
    async fn emit(&self, event: &ExportEvent) -> Result<()> {
        let body = serde_json::to_vec(event)?;
        self.target
            .put(&event_object_key(event), body, "application/json")
            .await
    }

    fn name(&self) -> &str {
        self.target.provider()
    }
}

/// Build a storage sink from config. Returns None (with a warning) if the
/// provider is not compiled into this binary.
pub fn build_sink(config: &StorageConfig) -> Result<Option<Box<dyn ExportSink>>> {
    let provider = config.provider.trim().to_ascii_lowercase();
    if !provider_available(&provider) {
        tracing::warn!(
            "Storage provider '{provider}' is not available. Supported: s3, gcs, azure, local; \
             cloud providers need the matching feature (e.g., --features export-s3)."
        );
        return Ok(None);
    }
    let target = ObjectTarget::from_config(config)?;
    Ok(Some(Box::new(ObjectStorageSink::new(target))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::EventKind;
    use chrono::TimeZone;
    use std::sync::Arc;

    fn event() -> ExportEvent {
        ExportEvent {
            kind: EventKind::PrAnalyzed,
            repo: "o/r".into(),
            timestamp: chrono::Utc.with_ymd_and_hms(2026, 3, 9, 12, 0, 0).unwrap(),
            data: serde_json::json!({"n": 1}),
        }
    }

    #[test]
    fn key_is_date_partitioned() {
        let e = event();
        assert_eq!(
            event_object_key(&e),
            format!(
                "2026/03/09/PrAnalyzed-{}.json",
                e.timestamp.timestamp_millis()
            )
        );
    }

    #[tokio::test]
    async fn sink_writes_json_under_prefix() {
        let store = Arc::new(object_store::memory::InMemory::new());
        let sink = ObjectStorageSink::new(ObjectTarget::from_store(
            store.clone(),
            "memory",
            "exports",
            None,
        ));
        let e = event();
        sink.emit(&e).await.unwrap();
        let path =
            object_store::path::Path::parse(format!("exports/{}", event_object_key(&e))).unwrap();
        let got = object_store::ObjectStoreExt::get(store.as_ref(), &path)
            .await
            .unwrap();
        assert_eq!(
            got.attributes
                .get(&object_store::Attribute::ContentType)
                .map(|v| v.to_string())
                .as_deref(),
            Some("application/json")
        );
        let v: serde_json::Value = serde_json::from_slice(&got.bytes().await.unwrap()).unwrap();
        assert_eq!(v["repo"], "o/r");
        assert_eq!(sink.name(), "memory");
    }

    #[test]
    fn unknown_provider_is_skipped() {
        let cfg = StorageConfig {
            provider: "ftp".into(),
            ..Default::default()
        };
        assert!(build_sink(&cfg).unwrap().is_none());
    }
}
