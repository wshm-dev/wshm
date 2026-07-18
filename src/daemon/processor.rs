use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{mpsc, Semaphore};
use tracing::{error, info, warn};

/// Maximum concurrent event processing tasks.
const MAX_CONCURRENT_TASKS: usize = 5;

use super::commands;
use super::memory;
use super::{DaemonState, MultiDaemonState};
use crate::cli::{PrArgs, TriageArgs};
use crate::github::sync as gh_sync;
use crate::pipelines;

/// Tracks which issue/PR numbers are currently being processed to prevent concurrent duplicates.
/// Key is (repo_slug, number) for multi-repo isolation, or ("", number) for single-repo.
///
/// Uses `std::sync::Mutex` so the in-flight key can be released by a
/// `Drop` guard (RAII) — the previous async-only release path leaked the
/// key permanently when `process_event` panicked, silently quarantining
/// the affected issue/PR until daemon restart.
type InFlight = Arc<StdMutex<HashSet<(String, u64)>>>;

/// Removes its (slug, number) key from the shared in-flight set on drop,
/// regardless of whether the surrounding future completes normally,
/// returns Err, or panics. The key is inserted by [`reserve_in_flight`];
/// constructing the guard outside that helper is a logic error.
struct InFlightGuard {
    map: InFlight,
    key: (String, u64),
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = self.map.lock() {
            set.remove(&self.key);
        }
    }
}

/// Try to reserve a (slug, number) slot in the in-flight set. Returns
/// `Some(guard)` on success — drop it (or let it fall out of scope) to
/// release the slot — or `None` if another task already holds it.
fn reserve_in_flight(map: &InFlight, slug: &str, number: u64) -> Option<InFlightGuard> {
    let key = (slug.to_string(), number);
    let inserted = match map.lock() {
        Ok(mut set) => set.insert(key.clone()),
        Err(_) => return None, // poisoned mutex — fail closed
    };
    if !inserted {
        return None;
    }
    Some(InFlightGuard {
        map: Arc::clone(map),
        key,
    })
}

#[derive(Debug, Clone)]
pub struct WebhookEvent {
    pub id: i64,
    pub event_type: String,
    pub action: String,
    pub number: Option<u64>,
    pub payload: String,
}

pub async fn run(state: Arc<DaemonState>, mut rx: mpsc::Receiver<WebhookEvent>) {
    info!("Event processor started (max {MAX_CONCURRENT_TASKS} concurrent)");
    let in_flight: InFlight = Arc::new(StdMutex::new(HashSet::new()));
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS));

    while let Some(event) = rx.recv().await {
        let state = Arc::clone(&state);
        let in_flight = Arc::clone(&in_flight);
        let permit = Arc::clone(&semaphore);
        tokio::spawn(async move {
            let _permit = permit.acquire().await;
            process_guarded(&state, &event, &in_flight, "").await;
        });
    }

    info!("Event processor stopped");
}

/// Multi-repo processor: events are tagged with repo slug.
pub async fn run_multi(
    multi: Arc<MultiDaemonState>,
    mut rx: mpsc::Receiver<(String, WebhookEvent)>,
) {
    info!("Multi-repo event processor started (max {MAX_CONCURRENT_TASKS} concurrent)");
    let in_flight: InFlight = Arc::new(StdMutex::new(HashSet::new()));
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_TASKS));

    while let Some((slug, event)) = rx.recv().await {
        let state = {
            let repos = multi.repos.read().await;
            match repos.get(&slug) {
                Some(s) => Arc::clone(s),
                None => {
                    error!("No state for repo '{slug}', dropping event id={}", event.id);
                    continue;
                }
            }
        };
        let in_flight = Arc::clone(&in_flight);
        let slug = slug.clone();
        let permit = Arc::clone(&semaphore);
        tokio::spawn(async move {
            let _permit = permit.acquire().await;
            process_guarded(&state, &event, &in_flight, &slug).await;
        });
    }

    info!("Multi-repo event processor stopped");
}

/// Guard against concurrent processing of the same (repo, issue/PR
/// number). The reservation is RAII so the slot is released even if
/// `process_event` panics (without a guard the key would leak forever
/// and the affected issue would be silently quarantined until restart).
async fn process_guarded(
    state: &DaemonState,
    event: &WebhookEvent,
    in_flight: &InFlight,
    slug: &str,
) {
    if let Some(number) = event.number {
        let _guard = match reserve_in_flight(in_flight, slug, number) {
            Some(g) => g,
            None => {
                warn!(
                    "Skipping event id={} for #{number} (already in-flight)",
                    event.id
                );
                return;
            }
        };
        process_event(state, event).await;
        // _guard drops here, releasing the slot. Drop also fires on panic.
    } else {
        process_event(state, event).await;
    }
}

async fn process_event(state: &DaemonState, event: &WebhookEvent) {
    info!(
        "Processing event id={} type={}.{}",
        event.id, event.event_type, event.action
    );

    // Mark as processing
    if let Err(e) = state.db.update_event_status(event.id, "processing", None) {
        error!("Failed to update event status: {e}");
        return;
    }

    let result = match event.event_type.as_str() {
        "issues" => handle_issue(state, event).await,
        "pull_request" => handle_pull_request(state, event).await,
        "issue_comment" => handle_comment(state, event).await,
        _ => {
            info!("Unknown event type: {}", event.event_type);
            Ok(())
        }
    };

    match result {
        Ok(()) => {
            info!("Event id={} processed successfully", event.id);
            if let Err(e) = state.db.update_event_status(event.id, "done", None) {
                error!("Failed to update event status to done: {e}");
            }
        }
        Err(e) => {
            error!("Event id={} failed: {e}", event.id);
            let err_msg = format!("{e:#}");
            if let Err(e2) = state
                .db
                .update_event_status(event.id, "failed", Some(&err_msg))
            {
                error!("Failed to update event status to failed: {e2}");
            }
        }
    }
}

async fn handle_issue(state: &DaemonState, event: &WebhookEvent) -> anyhow::Result<()> {
    let number = event.number;
    info!("Handling issue event: number={number:?}");

    // Skip if already triaged (prevents AI credit exhaustion via issue spam)
    if let Some(n) = number {
        if state.config.issues_blacklist.contains(&n) {
            info!("Skipping blacklisted issue #{n}");
            return Ok(());
        }
        match state.db.is_triaged(n) {
            Ok(true) => {
                info!("Issue #{n} already triaged, skipping");
                return Ok(());
            }
            Ok(false) => {}
            Err(e) => {
                // A transient DB/IO error must NOT fall through to a
                // re-triage: that would burn AI credits on every error.
                // Treat an unknown triage state as already-triaged (skip).
                warn!("is_triaged check failed for #{n}, skipping to avoid re-triage: {e:#}");
                return Ok(());
            }
        }
    }

    let features = state.features();
    if !features.collect_issues {
        info!(
            "[{}] collect_issues disabled — skipping sync for issue {number:?}",
            state.config.repo_slug()
        );
        return Ok(());
    }
    // Force sync issues (bypass throttle — we know there's a new event)
    gh_sync::sync_issues_now(&state.gh(), state.db.as_ref()).await?;

    // The list endpoint lags issue creation by a few seconds (replicated
    // index), so a sync right after `issues.opened` can miss the brand-new
    // issue, which then surfaces as "Issue #N not found in cache" at triage.
    // Fetch it directly from the canonical single-issue endpoint to guarantee
    // it is cached before we triage.
    if let Some(n) = number {
        if matches!(state.db.get_issue(n), Ok(None)) {
            match state.gh().fetch_issue(n).await {
                Ok(Some(issue)) => {
                    if let Err(e) = state.db.upsert_issue(&issue) {
                        warn!("Failed to cache issue #{n} after direct fetch: {e}");
                    }
                }
                Ok(None) => {
                    info!(
                        "[{}] issue #{n} not returned by GitHub (deleted, transferred, or a PR) — skipping",
                        state.config.repo_slug()
                    );
                    return Ok(());
                }
                Err(e) => warn!("Direct fetch of issue #{n} failed: {e:#}"),
            }
        }
    }

    if !features.triage_issues {
        info!(
            "[{}] triage_issues disabled — issue {number:?} synced but not triaged",
            state.config.repo_slug()
        );
        return Ok(());
    }

    // Apply per-action filters before consuming AI credits.
    if let Some(n) = number {
        if let Ok(Some(issue)) = state.db.get_issue(n) {
            if features.filters.is_author_skipped(issue.author.as_deref()) {
                info!(
                    "[{}] issue #{n} skipped (author '{}' in skip_authors)",
                    state.config.repo_slug(),
                    issue.author.as_deref().unwrap_or("?")
                );
                return Ok(());
            }
            if !features.filters.issue_labels_pass_triage(&issue.labels) {
                info!(
                    "[{}] issue #{n} skipped (labels filter)",
                    state.config.repo_slug()
                );
                return Ok(());
            }
            if !features.filters.issue_age_ok(&issue.created_at) {
                info!(
                    "[{}] issue #{n} skipped (older than {} days)",
                    state.config.repo_slug(),
                    features.filters.triage_max_age_days
                );
                return Ok(());
            }
        }
    }

    // Run triage pipeline. `apply` controls whether wshm posts a triage
    // comment on GitHub; we keep the legacy semantic (state.apply()) here
    // because triage_issues already gated the AI run itself.
    let args = TriageArgs {
        issue: number,
        apply: state.apply(),
        retriage: false,
    };

    pipelines::triage::run(
        &state.config,
        state.db.as_ref(),
        &state.gh(),
        &args,
        pipelines::triage::OutputFormat::Text,
        None,
    )
    .await?;

    // Store in ICM if enabled
    if state.config.daemon.icm_enabled {
        if let Some(n) = number {
            if let Ok(Some(triage)) = state.db.get_triage_result(n) {
                memory::store_triage(
                    &state.config,
                    n,
                    &triage.category,
                    triage.confidence,
                    triage.summary.as_deref().unwrap_or(""),
                )
                .await;
            }
        }
    }

    Ok(())
}

async fn handle_pull_request(state: &DaemonState, event: &WebhookEvent) -> anyhow::Result<()> {
    let number = event.number;
    info!("Handling pull_request event: number={number:?}");

    // Skip if blacklisted or already analyzed (prevent AI credit exhaustion)
    if let Some(n) = number {
        if state.config.prs_blacklist.contains(&n) {
            info!("Skipping blacklisted PR #{n}");
            return Ok(());
        }
        if let Ok(Some(analysis)) = state.db.get_pr_analysis(n) {
            if event.action == "opened" {
                info!("PR #{n} already analyzed, skipping");
                return Ok(());
            }
            // For synchronize: throttle re-analysis (max once per 5 min)
            if let Ok(last) = analysis
                .analyzed_at
                .parse::<chrono::DateTime<chrono::Utc>>()
            {
                let elapsed = chrono::Utc::now().signed_duration_since(last);
                if elapsed.num_minutes() < 5 {
                    info!(
                        "PR #{n} analyzed {}s ago, throttling re-analysis",
                        elapsed.num_seconds()
                    );
                    return Ok(());
                }
            }
        }
    }

    let features = state.features();
    if !features.collect_prs {
        info!(
            "[{}] collect_prs disabled — skipping sync for PR {number:?}",
            state.config.repo_slug()
        );
        return Ok(());
    }
    // Force sync pulls (bypass throttle — we know there's a new event)
    gh_sync::sync_pulls_now(&state.gh(), state.db.as_ref()).await?;

    // The list endpoint lags PR creation by a few seconds (replicated index),
    // so a sync right after `pull_request.opened` can miss the brand-new PR,
    // which then surfaces as "PR #N not found in cache" at analysis. Fetch it
    // directly from the canonical single-PR endpoint to guarantee it is cached.
    if let Some(n) = number {
        if matches!(state.db.get_pull(n), Ok(None)) {
            match state.gh().fetch_pull(n).await {
                Ok(Some(pr)) => {
                    if let Err(e) = state.db.upsert_pull(&pr) {
                        warn!("Failed to cache PR #{n} after direct fetch: {e}");
                    }
                }
                Ok(None) => {
                    info!(
                        "[{}] PR #{n} not returned by GitHub (deleted or transferred) — skipping",
                        state.config.repo_slug()
                    );
                    return Ok(());
                }
                Err(e) => warn!("Direct fetch of PR #{n} failed: {e:#}"),
            }
        }
    }

    if !features.analyze_prs {
        info!(
            "[{}] analyze_prs disabled — PR {number:?} synced but not analyzed",
            state.config.repo_slug()
        );
        return Ok(());
    }

    // Apply per-action filters.
    if let Some(n) = number {
        if let Ok(Some(pr)) = state.db.get_pull(n) {
            if features.filters.is_author_skipped(pr.author.as_deref()) {
                info!(
                    "[{}] PR #{n} skipped (author '{}' in skip_authors)",
                    state.config.repo_slug(),
                    pr.author.as_deref().unwrap_or("?")
                );
                return Ok(());
            }
            if !features.filters.branch_allowed(pr.base_ref.as_deref()) {
                info!(
                    "[{}] PR #{n} skipped (base '{}' not in target_branches)",
                    state.config.repo_slug(),
                    pr.base_ref.as_deref().unwrap_or("?")
                );
                return Ok(());
            }
            // skip_drafts not applied here yet — Issue model in DB doesn't
            // currently expose draft status; left as TODO once the field
            // is added to PullRequest.
        }
    }

    // Run PR analysis pipeline
    let args = PrArgs {
        pr: number,
        apply: state.apply(),
    };

    pipelines::pr_analysis::run(
        &state.config,
        state.db.as_ref(),
        &state.gh(),
        &args,
        false,
        None,
    )
    .await?;

    // Store in ICM if enabled
    if state.config.daemon.icm_enabled {
        if let Some(n) = number {
            if let Ok(Some(analysis)) = state.db.get_pr_analysis(n) {
                memory::store_pr_analysis(
                    &state.config,
                    n,
                    &analysis.pr_type,
                    &analysis.risk_level,
                    &analysis.summary,
                )
                .await;
            }
        }
    }

    Ok(())
}

async fn handle_comment(state: &DaemonState, event: &WebhookEvent) -> anyhow::Result<()> {
    let number = match event.number {
        Some(n) => n,
        None => return Ok(()),
    };

    // Parse the comment body from the payload
    let payload: serde_json::Value = serde_json::from_str(&event.payload)?;
    let comment_body = payload
        .get("comment")
        .and_then(|c| c.get("body"))
        .and_then(|b| b.as_str())
        .unwrap_or("");

    // Ignore our own comments (prevent infinite loops)
    let sender = payload
        .get("sender")
        .and_then(|s| s.get("login"))
        .and_then(|l| l.as_str())
        .unwrap_or("");
    let comment_marker = &state.config.branding.comment_marker();
    if comment_body.contains(comment_marker) || sender == "github-actions[bot]" {
        info!("Ignoring self-comment on #{number} by {sender}");
        return Ok(());
    }

    // Check if this is a slash command
    let cmd = match commands::parse(comment_body, &state.config.branding.command_prefix) {
        Some(c) => c,
        None => return Ok(()),
    };

    // Extract commenter username
    let triggered_by = payload
        .get("comment")
        .and_then(|c| c.get("user"))
        .and_then(|u| u.get("login"))
        .and_then(|l| l.as_str())
        .unwrap_or("unknown");

    info!("Slash command on #{number} by {triggered_by}: {cmd:?}");

    // Detect if this is a PR (issue_comment fires for both issues and PRs)
    let is_pr = payload
        .get("issue")
        .and_then(|i| i.get("pull_request"))
        .is_some();

    // Execute the command
    let response = commands::execute(
        &cmd,
        number,
        is_pr,
        &state.config,
        state.db.as_ref(),
        &state.gh(),
        state.apply(),
        Some(triggered_by),
    )
    .await?;

    // Post response as a comment
    if state.apply() {
        state.gh().comment_issue(number, &response).await?;
        info!("Posted slash command response on #{number}");
    } else {
        info!("Dry-run slash command response for #{number}: {response}");
    }

    Ok(())
}
