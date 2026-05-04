//! In-memory daemon log buffer.
//!
//! Provides a [`tracing_subscriber::Layer`] that captures every log event
//! into a ring buffer (size: [`MAX_ENTRIES`]) so the web UI can render the
//! tail of the daemon's log without `kubectl logs` access. A broadcast
//! channel is also exposed for future SSE/WebSocket streaming consumers.
//!
//! Wire it from `main.rs` like:
//!
//! ```rust,ignore
//! let logs = Arc::new(LogBuffer::new());
//! let log_layer = LogLayer::new(Arc::clone(&logs));
//! tracing_subscriber::registry()
//!     .with(EnvFilter::from_default_env())
//!     .with(tracing_subscriber::fmt::layer())
//!     .with(log_layer)
//!     .init();
//! ```

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::{broadcast, Mutex};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// Process-wide log buffer instance, populated by [`install_global`] at
/// startup and read by `daemon::run_multi` to wire `WebState.logs`.
static GLOBAL: OnceLock<Arc<LogBuffer>> = OnceLock::new();

/// Install the process-wide log buffer if not already set, then return a
/// handle. Idempotent — subsequent calls return the existing buffer so the
/// tracing layer registered with it stays alive across reinit attempts.
pub fn install_global() -> Arc<LogBuffer> {
    GLOBAL.get_or_init(|| Arc::new(LogBuffer::new())).clone()
}

/// Returns the process-wide log buffer if [`install_global`] was called.
pub fn global() -> Option<Arc<LogBuffer>> {
    GLOBAL.get().cloned()
}

/// Number of log lines retained in memory. ~5000 lines @ 200B each ≈ 1 MB.
pub const MAX_ENTRIES: usize = 5000;

/// Broadcast channel capacity for live tailing. Slow consumers drop messages.
const BROADCAST_CAPACITY: usize = 256;

/// One captured log line in a form usable from the web UI.
#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    /// Monotonic sequence number; clients use it for incremental polling.
    pub id: u64,
    /// RFC3339 timestamp.
    pub at: String,
    /// `INFO`, `WARN`, ...
    pub level: &'static str,
    /// Module path, e.g. `wshm_core::daemon::scheduler`.
    pub target: String,
    /// Rendered fields including the message.
    pub message: String,
}

/// Shared log store fed by [`LogLayer`] and queried by handlers.
pub struct LogBuffer {
    entries: Mutex<VecDeque<LogEntry>>,
    next_id: AtomicU64,
    tx: broadcast::Sender<LogEntry>,
}

impl LogBuffer {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            entries: Mutex::new(VecDeque::with_capacity(MAX_ENTRIES)),
            next_id: AtomicU64::new(1),
            tx,
        }
    }

    /// Subscribe to live log events. Each call returns an independent receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.tx.subscribe()
    }

    /// Read the most recent entries, optionally only those after `since` (id)
    /// and matching/exceeding `min_level`. Pass `None` to skip filters. The
    /// returned vec is in insertion order (oldest first).
    pub async fn snapshot(
        &self,
        tail: Option<usize>,
        since: Option<u64>,
        min_level: Option<Level>,
    ) -> Vec<LogEntry> {
        let entries = self.entries.lock().await;
        let mut out: Vec<LogEntry> = entries
            .iter()
            .filter(|e| since.is_none_or(|s| e.id > s))
            .filter(|e| {
                min_level.is_none_or(|lvl| level_severity(e.level) >= level_severity_of(lvl))
            })
            .cloned()
            .collect();
        if let Some(n) = tail {
            if out.len() > n {
                let cut = out.len() - n;
                out.drain(..cut);
            }
        }
        out
    }
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracing Layer that pushes events into a [`LogBuffer`].
pub struct LogLayer {
    buffer: Arc<LogBuffer>,
}

impl LogLayer {
    pub fn new(buffer: Arc<LogBuffer>) -> Self {
        Self { buffer }
    }
}

impl<S> Layer<S> for LogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let level = match *metadata.level() {
            Level::ERROR => "ERROR",
            Level::WARN => "WARN",
            Level::INFO => "INFO",
            Level::DEBUG => "DEBUG",
            Level::TRACE => "TRACE",
        };
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let id = self.buffer.next_id.fetch_add(1, Ordering::Relaxed);
        let entry = LogEntry {
            id,
            at: chrono::Utc::now().to_rfc3339(),
            level,
            target: metadata.target().to_string(),
            message: visitor.message,
        };

        // Push to ring buffer; drop oldest when full.
        // Use try_lock to avoid blocking the tracing path. If contended we
        // simply drop the entry — keeps the layer lock-free in the common
        // path and avoids reentrancy issues.
        if let Ok(mut entries) = self.buffer.entries.try_lock() {
            if entries.len() >= MAX_ENTRIES {
                entries.pop_front();
            }
            entries.push_back(entry.clone());
        }

        let _ = self.buffer.tx.send(entry);
    }
}

/// `tracing::Visit` implementation that concatenates all recorded fields into
/// a single string, with the `message` field first.
#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message.push_str(value);
        } else {
            self.append_kv(field.name(), value);
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        if field.name() == "message" {
            if !self.message.is_empty() {
                self.message.push(' ');
            }
            self.message.push_str(rendered.trim_matches('"'));
        } else {
            self.append_kv(field.name(), &rendered);
        }
    }
}

impl MessageVisitor {
    fn append_kv(&mut self, name: &str, value: &str) {
        if !self.message.is_empty() {
            self.message.push(' ');
        }
        self.message.push_str(name);
        self.message.push('=');
        self.message.push_str(value);
    }
}

/// Higher value = more severe. Used so a `min_level=WARN` filter keeps both
/// WARN and ERROR but drops INFO/DEBUG/TRACE.
fn level_severity(level: &str) -> u8 {
    match level {
        "ERROR" => 5,
        "WARN" => 4,
        "INFO" => 3,
        "DEBUG" => 2,
        "TRACE" => 1,
        _ => 0,
    }
}

fn level_severity_of(level: Level) -> u8 {
    match level {
        Level::ERROR => 5,
        Level::WARN => 4,
        Level::INFO => 3,
        Level::DEBUG => 2,
        Level::TRACE => 1,
    }
}

/// Parse a level string into a [`Level`]. Used by the API handler.
pub fn parse_level(s: &str) -> Option<Level> {
    match s.to_ascii_uppercase().as_str() {
        "ERROR" => Some(Level::ERROR),
        "WARN" => Some(Level::WARN),
        "INFO" => Some(Level::INFO),
        "DEBUG" => Some(Level::DEBUG),
        "TRACE" => Some(Level::TRACE),
        _ => None,
    }
}
