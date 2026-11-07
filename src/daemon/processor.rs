use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{mpsc, Semaphore};
use tracing::{error, info, warn};

/// Maximum concurrent event processing tasks.
const MAX_CONCURRENT_TASKS: usize = 5;

use super::commands;
use super::{DaemonState, MultiDaemonState};
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

    // Every cache write below (direct fetch or the sync it triggers) is
    // attributed to the webhook in the change history.
    let result = crate::db::history::scoped(crate::db::history::SOURCE_WEBHOOK, async {
        match event.event_type.as_str() {
            "issues" => handle_issue(state, event).await,
            "pull_request" => handle_pull_request(state, event).await,
            "pull_request_review" => handle_pull_request_review(state, event).await,
            "issue_comment" => handle_comment(state, event).await,
            _ => {
                info!("Unknown event type: {}", event.event_type);
                Ok(())
            }
        }
    })
    .await;

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

    // A collaborator's approval label (#235): recorded bound to the content
    // of the event, then the triage re-runs once for it.
    let approved = event.action == "labeled"
        && match serde_json::from_str::<serde_json::Value>(&event.payload) {
            Ok(p) => crate::ai::guard::on_issue_labeled(
                &state.config,
                state.db.as_ref(),
                &*state.gh(),
                &p,
            )
            .await
            .is_some(),
            Err(_) => false,
        };

    if event.action == "labeled" && !approved {
        return Ok(());
    }

    // Skip if already triaged (prevents AI credit exhaustion via issue spam)
    if let Some(n) = number {
        if state.config.issues_blacklist.contains(&n) {
            info!("Skipping blacklisted issue #{n}");
            return Ok(());
        }
        match state.db.is_triaged(n) {
            Ok(true) if !approved => {
                info!("Issue #{n} already triaged, skipping");
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => {
                // A transient DB/IO error must NOT fall through to a
                // re-triage: that would burn AI credits on every error.
                // Treat an unknown triage state as already-triaged (skip).
                warn!("is_triaged check failed for #{n}, skipping to avoid re-triage: {e:#}");
                return Ok(());
            }
        }
    }

    // Everything after the admission checks above is a pipeline run: the
    // repo's template (default `standard`, which mirrors the former inline
    // flow and reads the repo toggles as step conditions), recorded per step.
    let ctx = run_context(state, event, pipelines::engine::ItemKind::Issue);
    pipelines::engine::run_item(&ctx).await
}

async fn handle_pull_request(state: &DaemonState, event: &WebhookEvent) -> anyhow::Result<()> {
    let number = event.number;
    info!("Handling pull_request event: number={number:?}");

    // Closed or merged: remove its diagram page (`[pr.review_diagrams]`),
    // refresh the cached state, and stop there (nothing to analyze or review
    // on a closed PR).
    if event.action == "closed" {
        if let Some(n) = number {
            crate::diagrams::cleanup_pr(&state.config, state.db.as_ref(), state.gh().as_ref(), n)
                .await;
        }
        // Refresh just this PR (state closed/merged), not a full sync: every
        // merge would otherwise re-list every PR of the repo.
        if let (Some(n), true) = (number, state.features().collect_prs) {
            match state.gh().fetch_pull(n).await {
                Ok(Some(pr)) => {
                    if let Err(e) = state.db.upsert_pull(&pr) {
                        warn!("Failed to cache closed PR #{n}: {e}");
                    }
                }
                Ok(None) => {}
                Err(e) => warn!("Could not refresh closed PR #{n}: {e:#}"),
            }
        }
        return Ok(());
    }

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

    // Everything after the admission checks above is a pipeline run: the
    // repo's template (default `standard`, which mirrors the former inline
    // flow and reads the repo toggles as step conditions), recorded per step.
    let ctx = run_context(state, event, pipelines::engine::ItemKind::Pr);
    pipelines::engine::run_item(&ctx).await
}

/// A review was submitted or dismissed: re-run only the template's
/// `risk`, `approval` and `status_check` steps (no AI call, no review
/// round), so a pending status clears once the approvals are there.
async fn handle_pull_request_review(
    state: &DaemonState,
    event: &WebhookEvent,
) -> anyhow::Result<()> {
    let Some(n) = event.number else {
        return Ok(());
    };
    if state.config.prs_blacklist.contains(&n) {
        info!("Skipping blacklisted PR #{n}");
        return Ok(());
    }
    let ctx = run_context(state, event, pipelines::engine::ItemKind::Pr);
    pipelines::engine::run_kinds(
        &ctx,
        pipelines::engine::APPROVAL_KINDS,
        pipelines::engine::ExecOptions {
            record_only_if_active: true,
            ..Default::default()
        },
    )
    .await
    .into_result()
}

/// Pipeline context for an issue / PR event, with the repo's live apply
/// mode and toggles.
fn run_context(
    state: &DaemonState,
    event: &WebhookEvent,
    kind: pipelines::engine::ItemKind,
) -> pipelines::engine::RunContext {
    pipelines::engine::RunContext::new(
        Arc::clone(&state.config),
        Arc::clone(&state.db),
        state.gh(),
        state.apply(),
        state.features(),
        pipelines::engine::ItemRef {
            kind,
            number: event.number,
        },
        format!("{}.{}", event.event_type, event.action),
    )
    .with_event_id(event.id)
    .with_payload_info(
        serde_json::from_str::<serde_json::Value>(&event.payload)
            .ok()
            .and_then(|v| pipelines::engine::ItemInfo::from_payload(kind, &v)),
    )
}

async fn handle_comment(state: &DaemonState, event: &WebhookEvent) -> anyhow::Result<()> {
    let number = match event.number {
        Some(n) => n,
        None => return Ok(()),
    };

    // Parse the comment body from the payload
    let mut payload: serde_json::Value = serde_json::from_str(&event.payload)?;
    // Forges whose deliveries cannot prove who wrote what (Azure DevOps):
    // the author and text are read back from the API by comment id.
    if let Some(c) = super::azure_hooks::comment_to_verify(&payload) {
        match state.gh().fetch_comment(&c).await {
            Ok(Some(found)) if found.number == number => {
                payload["comment"]["body"] = serde_json::json!(found.body);
                payload["comment"]["user"]["login"] = serde_json::json!(found.author);
                payload["sender"]["login"] = serde_json::json!(found.author);
            }
            Ok(_) => {
                info!("Ignoring comment {c} on #{number}: not found through the API");
                return Ok(());
            }
            Err(e) => {
                warn!("Ignoring comment {c} on #{number}: cannot read it back: {e:#}");
                return Ok(());
            }
        }
    }
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

    // Never execute what wshm's own account wrote (a review or reply can
    // echo a command injected through PR content). GitHub returns None
    // here and keeps relying on the marker check above.
    if let Some(me) = state.gh().self_username().await {
        if me.eq_ignore_ascii_case(triggered_by) || me.eq_ignore_ascii_case(sender) {
            info!("Ignoring command on #{number} posted by wshm's own account {me}");
            return Ok(());
        }
    }

    info!("Slash command on #{number} by {triggered_by}: {cmd:?}");

    // Detect if this is a PR (issue_comment fires for both issues and PRs)
    let is_pr = payload
        .get("issue")
        .and_then(|i| i.get("pull_request"))
        .is_some();

    // Execute the command. triage / analyze / review go through the
    // repo's pipeline (template + step conditions, recorded as a run with
    // trigger "command").
    let kind = if is_pr {
        pipelines::engine::ItemKind::Pr
    } else {
        pipelines::engine::ItemKind::Issue
    };
    let ctx = run_context(state, event, kind)
        .with_source(pipelines::engine::RunSource::Command)
        .with_trigger("command");
    let response = commands::execute(&cmd, is_pr, &ctx, Some(triggered_by)).await?;

    // Post response as a comment
    if state.apply() {
        if is_pr {
            state.gh().comment_pr(number, &response).await?;
        } else {
            state.gh().comment_issue(number, &response).await?;
        }
        info!("Posted slash command response on #{number}");
    } else {
        info!("Dry-run slash command response for #{number}: {response}");
    }

    Ok(())
}
