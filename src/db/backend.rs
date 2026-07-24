//! DatabaseBackend trait — abstraction over SQLite and PostgreSQL backends.

use anyhow::Result;

use crate::ai::schemas::IssueClassification;
use crate::db::events::WebhookEventRow;
use crate::db::issues::Issue;
use crate::db::pulls::{PrAnalysisRow, PullRequest};
use crate::db::search::SearchHit;
use crate::db::sync::SyncEntry;
use crate::db::triage::TriageResultRow;
use crate::db::usage::UsageCounts;

/// Unified interface for both SQLite and PostgreSQL database backends.
///
/// All methods mirror the existing `Database` (SQLite) implementation.
/// Implementations must be Send + Sync for use across async contexts.
pub trait DatabaseBackend: Send + Sync {
    // ── Issues ──────────────────────────────────────────────────

    fn upsert_issue(&self, issue: &Issue) -> Result<()>;
    fn batch_upsert_issues(&self, issues: &[Issue]) -> Result<()>;
    fn get_issue(&self, number: u64) -> Result<Option<Issue>>;
    fn get_open_issues(&self) -> Result<Vec<Issue>>;
    fn get_untriaged_issues(&self) -> Result<Vec<Issue>>;
    fn get_issues_needing_triage(
        &self,
        limit: usize,
        relabel_labels: &[String],
        no_labels_min_age_hours: u32,
    ) -> Result<Vec<Issue>>;
    fn merge_issue_labels(&self, number: u64, add: &[String], remove: &[String]) -> Result<()>;

    // ── Pull Requests ───────────────────────────────────────────

    fn upsert_pull(&self, pr: &PullRequest) -> Result<()>;
    fn batch_upsert_pulls(&self, pulls: &[PullRequest]) -> Result<()>;
    fn get_pull(&self, number: u64) -> Result<Option<PullRequest>>;
    fn get_open_pulls(&self) -> Result<Vec<PullRequest>>;
    fn get_unanalyzed_pulls(&self) -> Result<Vec<PullRequest>>;
    fn get_pr_analysis(&self, pr_number: u64) -> Result<Option<PrAnalysisRow>>;
    /// Batch loader: every PR analysis keyed by PR number, in one query.
    /// Used by the web handlers to avoid an N+1 `get_pr_analysis` per open
    /// PR. Default impl returns an empty map; SQLite overrides it.
    fn get_all_pr_analyses(&self) -> Result<std::collections::HashMap<u64, PrAnalysisRow>> {
        Ok(std::collections::HashMap::new())
    }
    /// Recently-closed pull requests (highest `updated_at` first), capped
    /// at `limit`. Backs the changelog view in TUI and the /api/v1/changelog
    /// endpoint.
    fn get_closed_pulls(&self, limit: usize) -> Result<Vec<PullRequest>>;

    // ── Triage ──────────────────────────────────────────────────

    fn upsert_triage_result(&self, result: &IssueClassification, issue_number: u64) -> Result<()>;
    /// Same as `upsert_triage_result` but also persists the content hash so
    /// the next batch can detect whether the issue changed and skip re-spending
    /// AI credits.
    fn upsert_triage_result_with_hash(
        &self,
        result: &IssueClassification,
        issue_number: u64,
        content_hash: Option<&str>,
    ) -> Result<()>;
    fn get_triage_result(&self, issue_number: u64) -> Result<Option<TriageResultRow>>;
    /// Batch loader: every triage result keyed by issue number. Used by the
    /// web handlers to avoid an N+1 `get_triage_result` per open issue.
    /// Default impl falls back to an empty map; the SQLite backend overrides
    /// it with a single query.
    fn get_all_triage_results(&self) -> Result<std::collections::HashMap<u64, TriageResultRow>> {
        Ok(std::collections::HashMap::new())
    }
    /// Batch loader: count of wshm-applied labels per issue, in one query.
    /// Default impl returns an empty map; SQLite overrides it.
    fn get_applied_label_counts(&self) -> Result<std::collections::HashMap<u64, usize>> {
        Ok(std::collections::HashMap::new())
    }
    /// Insert `triage_results` stubs for open issues that already carry
    /// wshm-managed labels on the forge but have no local row. Lets a
    /// fresh install / migration / wiped state.db avoid re-spending LLM
    /// credits on issues that are clearly already triaged. See
    /// [`Database::seed_triage_stubs_from_labels`] for details. Default
    /// impl returns 0 so a backend without label-aware seeding still
    /// compiles — the SQLite impl overrides it.
    fn seed_triage_stubs_from_labels(
        &self,
        managed_label_prefixes: &[String],
        grace_hours: u32,
    ) -> Result<u64> {
        let _ = (managed_label_prefixes, grace_hours);
        Ok(0)
    }
    fn get_stale_triage_results(&self, max_age_hours: u32) -> Result<Vec<TriageResultRow>>;
    fn get_wshm_applied_labels(&self, issue_number: u64) -> Result<Vec<String>>;
    fn recent_activity(&self, limit: usize) -> Result<Vec<TriageResultRow>>;
    fn is_triaged(&self, issue_number: u64) -> Result<bool>;

    // ── PR analysis ─────────────────────────────────────────────

    /// Open PRs that need (re)analysis: never analyzed OR content_hash changed.
    fn get_pulls_needing_analysis(&self) -> Result<Vec<PullRequest>>;
    /// Upsert one row into `pr_analyses`. Replaces the previous inline
    /// `with_conn(|conn| INSERT ... ON CONFLICT ...)` so callers can run
    /// against any backend that implements this trait.
    fn upsert_pr_analysis(&self, row: &PrAnalysisRow) -> Result<()>;

    /// Apply freshly-synced GitHub review decisions (PR number → decision)
    /// to open PRs; PRs absent from the map get their decision cleared.
    /// Backs the "To Validate" review-radar view. Default impl is a no-op
    /// returning 0 so backends without the columns keep compiling — the
    /// SQLite impl overrides it.
    fn set_review_decisions(
        &self,
        decisions: &std::collections::HashMap<u64, Option<String>>,
    ) -> Result<u64> {
        let _ = decisions;
        Ok(0)
    }

    /// Apply 👍 (+1) reaction counts to open PRs (number → count). Sizes the
    /// PR node in the label graph. Default no-op returning 0 so backends
    /// without the column keep compiling — real backends override it.
    fn set_pull_reactions(&self, reactions: &std::collections::HashMap<u64, u32>) -> Result<u64> {
        let _ = reactions;
        Ok(0)
    }

    // ── Admin / maintenance ─────────────────────────────────────

    /// Wipe every triage result and PR analysis. Used by the `revert` flow
    /// to undo all wshm-applied state before re-syncing from the forge.
    fn clear_triage_and_analyses(&self) -> Result<()>;

    // ── Sync Log ────────────────────────────────────────────────

    fn get_sync_entry(&self, table_name: &str) -> Result<Option<SyncEntry>>;
    fn update_sync_entry(
        &self,
        table_name: &str,
        last_synced_at: &str,
        etag: Option<&str>,
    ) -> Result<()>;

    // ── Webhook Events ──────────────────────────────────────────

    fn insert_webhook_event(
        &self,
        event_type: &str,
        action: &str,
        number: Option<u64>,
        payload: &str,
    ) -> Result<i64>;
    fn update_event_status(&self, id: i64, status: &str, error: Option<&str>) -> Result<()>;
    fn pending_event_count(&self) -> Result<u64>;
    fn cleanup_old_events(&self, days: u32) -> Result<u64>;
    fn get_pending_events(&self) -> Result<Vec<WebhookEventRow>>;

    // ── Search ──────────────────────────────────────────────────

    /// Full-text search across issues, pull requests, triage results and
    /// comments for this repo. The free-form `query` is sanitised inside
    /// each backend implementation (FTS5 prefix-AND on SQLite, websearch
    /// tsquery on Postgres) so callers pass user input as-is.
    ///
    /// Returns at most `limit` hits ordered best first. `limit` is clamped
    /// to [1, 500] inside each implementation to bound per-repo cost — the
    /// handler paginates the merged result set across repos.
    fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>>;

    // ── LLM accounting ──────────────────────────────────────────

    /// Record one successful LLM invocation so the Pro usage dashboard
    /// can show real call counts (vs cache-hit "triages" that touch no
    /// LLM). `kind` is a short label like `"triage"` or `"pr_analysis"`;
    /// `model` is the model identifier if known.
    ///
    /// Default impl is a no-op: OSS SQLite installs don't need the
    /// accounting and we don't want to slow the hot path with an extra
    /// write per LLM call. The Pro Postgres backend overrides this to
    /// insert into the `llm_invocations` table.
    fn record_llm_invocation(&self, kind: &str, model: Option<&str>) -> Result<()> {
        let _ = (kind, model);
        Ok(())
    }

    /// Roll-up of LLM invocation counts over the last 24h / 7d / 30d
    /// (plus all-time total and a per-kind breakdown) for this repo's
    /// backend. Default impl returns zeroes so OSS SQLite installs
    /// render an empty Pro usage dashboard rather than failing the
    /// route; only the Pro Postgres backend queries `llm_invocations`.
    fn usage_counts(&self) -> Result<UsageCounts> {
        Ok(UsageCounts::default())
    }

    // ── Escape hatch ────────────────────────────────────────────

    /// Downcast to the concrete SQLite `Database` if this backend is SQLite.
    /// Returns `None` for non-SQLite backends (e.g. Postgres).
    ///
    /// Use sparingly — every call site here is a feature that is silently
    /// unavailable on non-SQLite backends. Prefer adding the operation to
    /// this trait so all backends implement it. Today this hatch backs:
    /// the FTS search endpoint and a handful of ad-hoc Pro-only SQL queries
    /// that have not been ported to the trait yet.
    fn as_sqlite_db(&self) -> Option<&super::Database> {
        None
    }
}

/// Implement DatabaseBackend for the existing SQLite Database.
impl DatabaseBackend for super::Database {
    fn upsert_issue(&self, issue: &Issue) -> Result<()> {
        self.upsert_issue(issue)
    }

    fn batch_upsert_issues(&self, issues: &[Issue]) -> Result<()> {
        self.batch_upsert_issues(issues)
    }

    fn get_issue(&self, number: u64) -> Result<Option<Issue>> {
        self.get_issue(number)
    }

    fn get_open_issues(&self) -> Result<Vec<Issue>> {
        self.get_open_issues()
    }

    fn get_untriaged_issues(&self) -> Result<Vec<Issue>> {
        self.get_untriaged_issues()
    }

    fn get_issues_needing_triage(
        &self,
        limit: usize,
        relabel_labels: &[String],
        no_labels_min_age_hours: u32,
    ) -> Result<Vec<Issue>> {
        self.get_issues_needing_triage(limit, relabel_labels, no_labels_min_age_hours)
    }

    fn merge_issue_labels(&self, number: u64, add: &[String], remove: &[String]) -> Result<()> {
        self.merge_issue_labels(number, add, remove)
    }

    fn upsert_pull(&self, pr: &PullRequest) -> Result<()> {
        self.upsert_pull(pr)
    }

    fn batch_upsert_pulls(&self, pulls: &[PullRequest]) -> Result<()> {
        self.batch_upsert_pulls(pulls)
    }

    fn get_pull(&self, number: u64) -> Result<Option<PullRequest>> {
        self.get_pull(number)
    }

    fn get_open_pulls(&self) -> Result<Vec<PullRequest>> {
        self.get_open_pulls()
    }

    fn get_unanalyzed_pulls(&self) -> Result<Vec<PullRequest>> {
        self.get_unanalyzed_pulls()
    }

    fn get_pr_analysis(&self, pr_number: u64) -> Result<Option<PrAnalysisRow>> {
        self.get_pr_analysis(pr_number)
    }

    fn get_all_pr_analyses(&self) -> Result<std::collections::HashMap<u64, PrAnalysisRow>> {
        self.get_all_pr_analyses()
    }

    fn get_closed_pulls(&self, limit: usize) -> Result<Vec<PullRequest>> {
        self.get_closed_pulls(limit)
    }

    fn upsert_triage_result(&self, result: &IssueClassification, issue_number: u64) -> Result<()> {
        self.upsert_triage_result(result, issue_number)
    }

    fn upsert_triage_result_with_hash(
        &self,
        result: &IssueClassification,
        issue_number: u64,
        content_hash: Option<&str>,
    ) -> Result<()> {
        self.upsert_triage_result_with_hash(result, issue_number, content_hash)
    }

    fn get_triage_result(&self, issue_number: u64) -> Result<Option<TriageResultRow>> {
        self.get_triage_result(issue_number)
    }

    fn get_all_triage_results(&self) -> Result<std::collections::HashMap<u64, TriageResultRow>> {
        self.get_all_triage_results()
    }

    fn get_applied_label_counts(&self) -> Result<std::collections::HashMap<u64, usize>> {
        self.get_applied_label_counts()
    }

    fn seed_triage_stubs_from_labels(
        &self,
        managed_label_prefixes: &[String],
        grace_hours: u32,
    ) -> Result<u64> {
        self.seed_triage_stubs_from_labels(managed_label_prefixes, grace_hours)
    }

    fn get_stale_triage_results(&self, max_age_hours: u32) -> Result<Vec<TriageResultRow>> {
        self.get_stale_triage_results(max_age_hours)
    }

    fn get_wshm_applied_labels(&self, issue_number: u64) -> Result<Vec<String>> {
        self.get_wshm_applied_labels(issue_number)
    }

    fn recent_activity(&self, limit: usize) -> Result<Vec<TriageResultRow>> {
        self.recent_activity(limit)
    }

    fn is_triaged(&self, issue_number: u64) -> Result<bool> {
        self.is_triaged(issue_number)
    }

    fn get_sync_entry(&self, table_name: &str) -> Result<Option<SyncEntry>> {
        self.get_sync_entry(table_name)
    }

    fn update_sync_entry(
        &self,
        table_name: &str,
        last_synced_at: &str,
        etag: Option<&str>,
    ) -> Result<()> {
        self.update_sync_entry(table_name, last_synced_at, etag)
    }

    fn insert_webhook_event(
        &self,
        event_type: &str,
        action: &str,
        number: Option<u64>,
        payload: &str,
    ) -> Result<i64> {
        self.insert_webhook_event(event_type, action, number, payload)
    }

    fn update_event_status(&self, id: i64, status: &str, error: Option<&str>) -> Result<()> {
        self.update_event_status(id, status, error)
    }

    fn pending_event_count(&self) -> Result<u64> {
        self.pending_event_count()
    }

    fn cleanup_old_events(&self, days: u32) -> Result<u64> {
        self.cleanup_old_events(days)
    }

    fn get_pending_events(&self) -> Result<Vec<WebhookEventRow>> {
        self.get_pending_events()
    }

    fn search_fts(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        // SQLite path applies the FTS5-specific sanitisation (token*-AND);
        // Postgres backends sanitise differently inside their impl, so we
        // keep the rule here rather than in shared code.
        match super::search::sanitize_query(query) {
            Some(match_expr) => self.search_fts(&match_expr, limit),
            None => Ok(Vec::new()),
        }
    }

    fn get_pulls_needing_analysis(&self) -> Result<Vec<PullRequest>> {
        self.get_pulls_needing_analysis()
    }

    fn upsert_pr_analysis(&self, row: &PrAnalysisRow) -> Result<()> {
        self.upsert_pr_analysis(row)
    }

    fn set_review_decisions(
        &self,
        decisions: &std::collections::HashMap<u64, Option<String>>,
    ) -> Result<u64> {
        self.set_review_decisions(decisions)
    }

    fn set_pull_reactions(&self, reactions: &std::collections::HashMap<u64, u32>) -> Result<u64> {
        self.set_pull_reactions(reactions)
    }

    fn clear_triage_and_analyses(&self) -> Result<()> {
        self.clear_triage_and_analyses()
    }

    fn as_sqlite_db(&self) -> Option<&super::Database> {
        Some(self)
    }
}
