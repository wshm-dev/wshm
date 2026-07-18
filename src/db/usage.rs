//! LLM usage accounting summary for the Pro dashboard.
//!
//! The Pro Postgres backend persists every successful LLM call into
//! `llm_invocations` via [`DatabaseBackend::record_llm_invocation`].
//! The Pro usage dashboard reads these counts back through
//! [`DatabaseBackend::usage_counts`].
//!
//! OSS SQLite installs are usage-tracking opt-out: the default impl in
//! the trait returns zeroes so the dashboard renders an empty state
//! instead of forcing a schema migration on every OSS user.

use serde::Serialize;

/// Roll-up of LLM call counts for one repo over the three windows the
/// Pro dashboard surfaces. Counts are returned per-kind (`triage`,
/// `pr_analysis`, ...) so the UI can break them down without re-querying.
#[derive(Debug, Default, Clone, Serialize)]
pub struct UsageCounts {
    pub total: u64,
    pub last_24h: u64,
    pub last_7d: u64,
    pub last_30d: u64,
    /// Per-kind breakdown for the same windows. Keys are the `kind`
    /// strings passed to `record_llm_invocation` (lower-case, snake_case).
    pub by_kind: Vec<UsageByKind>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct UsageByKind {
    pub kind: String,
    pub total: u64,
    pub last_24h: u64,
    pub last_7d: u64,
    pub last_30d: u64,
}
