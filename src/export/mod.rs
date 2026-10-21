pub mod database;
pub mod storage;
pub mod webhook;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{error, warn};

use crate::config::ExportConfig;

/// Event kinds emitted by pipelines.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum EventKind {
    IssueTriaged,
    PrAnalyzed,
    FixApplied,
    CommentPosted,
    PrMerged,
    ConflictResolved,
    SyncCompleted,
}

impl EventKind {
    pub fn as_str(&self) -> &str {
        match self {
            EventKind::IssueTriaged => "IssueTriaged",
            EventKind::PrAnalyzed => "PrAnalyzed",
            EventKind::FixApplied => "FixApplied",
            EventKind::CommentPosted => "CommentPosted",
            EventKind::PrMerged => "PrMerged",
            EventKind::ConflictResolved => "ConflictResolved",
            EventKind::SyncCompleted => "SyncCompleted",
        }
    }

    /// Check if this event kind matches a filter string (or "*" for all).
    pub fn matches_filter(&self, filter: &str) -> bool {
        filter == "*" || filter == self.as_str()
    }
}

/// An export event emitted after each pipeline action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportEvent {
    pub kind: EventKind,
    pub repo: String,
    pub timestamp: DateTime<Utc>,
    pub data: serde_json::Value,
}

/// Common interface for all export sinks.
#[async_trait]
pub trait ExportSink: Send + Sync {
    /// Send an event to this sink.
    async fn emit(&self, event: &ExportEvent) -> Result<()>;
    /// Human-readable name for logging.
    fn name(&self) -> &str;
}

/// Add a built sink, or log why it is skipped (misconfiguration).
fn push_or_skip(
    sinks: &mut Vec<Box<dyn ExportSink>>,
    kind: &str,
    provider: &str,
    built: Result<Option<Box<dyn ExportSink>>>,
) {
    match built {
        Ok(Some(sink)) => sinks.push(sink),
        Ok(None) => {}
        Err(e) => error!("{kind} export ({provider}) is misconfigured, skipping it: {e:#}"),
    }
}

/// Fan-out manager that dispatches events to all configured sinks.
pub struct ExportManager {
    sinks: Vec<Box<dyn ExportSink>>,
}

impl ExportManager {
    /// Build an ExportManager from config. Returns None if no sinks are configured.
    ///
    /// Webhooks are free (OSS). Cloud storage (S3/Azure/GCS) and database
    /// sinks (Elastic/OpenSearch/MongoDB/Postgres/MySQL) require a Pro license
    /// with the `exports` feature enabled.
    ///
    /// A sink that fails to build (missing bucket, invalid prefix...) is
    /// logged as an error and skipped, like an unavailable provider: a bad
    /// export config never prevents wshm from starting.
    pub fn from_config(config: &ExportConfig) -> Result<Option<Self>> {
        let mut sinks: Vec<Box<dyn ExportSink>> = Vec::new();

        // Webhooks (free, OSS)
        for wh in &config.webhooks {
            sinks.push(Box::new(webhook::WebhookSink::new(wh)));
        }

        // Cloud storage (Pro: exports feature)
        if let Some(ref storage) = config.storage {
            if !crate::pro_hooks::has_feature("exports") {
                warn!(
                    "Cloud storage export ({}) configured but requires wshm Pro. \
                     Skipping. Run `wshm login --license` to activate.",
                    storage.provider
                );
            } else {
                push_or_skip(
                    &mut sinks,
                    "Cloud storage",
                    &storage.provider,
                    storage::build_sink(storage),
                );
            }
        }

        // Database sinks (Pro: exports feature)
        if let Some(ref db) = config.database {
            if !crate::pro_hooks::has_feature("exports") {
                warn!(
                    "Database export ({}) configured but requires wshm Pro. \
                     Skipping. Run `wshm login --license` to activate.",
                    db.provider
                );
            } else {
                push_or_skip(
                    &mut sinks,
                    "Database",
                    &db.provider,
                    database::build_sink(db),
                );
            }
        }

        if sinks.is_empty() {
            Ok(None)
        } else {
            Ok(Some(Self { sinks }))
        }
    }

    /// Emit an event to all configured sinks. Errors are logged but don't stop other sinks.
    pub async fn emit(&self, event: &ExportEvent) -> Result<()> {
        for sink in &self.sinks {
            if let Err(e) = sink.emit(event).await {
                warn!("Export sink '{}' failed: {e:#}", sink.name());
            }
        }
        Ok(())
    }

    /// Number of configured sinks.
    pub fn sink_count(&self) -> usize {
        self.sinks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StorageConfig;

    #[test]
    fn misconfigured_sink_is_skipped_not_fatal() {
        let mut sinks: Vec<Box<dyn ExportSink>> = Vec::new();
        // `local` is always compiled in; without `dir` it fails to build.
        let bad = StorageConfig {
            provider: "local".into(),
            ..Default::default()
        };
        let built = storage::build_sink(&bad);
        assert!(built.is_err());
        push_or_skip(&mut sinks, "Cloud storage", &bad.provider, built);
        assert!(sinks.is_empty());

        let dir = std::env::temp_dir().join(format!("wshm-export-skip-{}", std::process::id()));
        let good = StorageConfig {
            provider: "local".into(),
            dir: Some(dir.to_string_lossy().into_owned()),
            ..Default::default()
        };
        push_or_skip(
            &mut sinks,
            "Cloud storage",
            "local",
            storage::build_sink(&good),
        );
        assert_eq!(sinks.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
