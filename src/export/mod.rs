pub mod database;
pub mod storage;
pub mod webhook;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

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

/// Fan-out manager that dispatches events to all configured sinks.
pub struct ExportManager {
    sinks: Vec<Box<dyn ExportSink>>,
}

impl ExportManager {
    /// Build an ExportManager from config. Returns None if no sinks are configured.
    pub fn from_config(config: &ExportConfig) -> Result<Option<Self>> {
        let mut sinks: Vec<Box<dyn ExportSink>> = Vec::new();

        // Webhooks
        for wh in &config.webhooks {
            sinks.push(Box::new(webhook::WebhookSink::new(wh)));
        }

        // Cloud storage
        if let Some(ref storage) = config.storage {
            if let Some(sink) = storage::build_sink(storage)? {
                sinks.push(sink);
            }
        }

        // Database
        if let Some(ref db) = config.database {
            if let Some(sink) = database::build_sink(db)? {
                sinks.push(sink);
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
