use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

use super::commands;
use super::memory;
use super::{DaemonState, MultiDaemonState};
use crate::cli::{PrArgs, TriageArgs};
use crate::github::sync as gh_sync;
use crate::pipelines;

/// Tracks which issue/PR numbers are currently being processed to prevent concurrent duplicates.
/// Key is (repo_slug, number) for multi-repo isolation, or ("", number) for single-repo.
type InFlight = Arc<Mutex<HashSet<(String, u64)>>>;

#[derive(Debug, Clone)]
pub struct WebhookEvent {
    pub id: i64,
    pub event_type: String,
    pub action: String,
    pub number: Option<u64>,
    pub payload: String,
}

pub async fn run(state: Arc<DaemonState>, mut rx: mpsc::Receiver<WebhookEvent>) {
    info!("Event processor started");
    let in_flight: InFlight = Arc::new(Mutex::new(HashSet::new()));

    while let Some(event) = rx.recv().await {
        let state = Arc::clone(&state);
        let in_flight = Arc::clone(&in_flight);
        tokio::spawn(async move {
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
    info!("Multi-repo event processor started");
    let in_flight: InFlight = Arc::new(Mutex::new(HashSet::new()));

    while let Some((slug, event)) = rx.recv().await {
        let state = match multi.repos.get(&slug) {
            Some(s) => Arc::clone(s),
            None => {
                error!("No state for repo '{slug}', dropping event id={}", event.id);
                continue;
            }
        };
        let in_flight = Arc::clone(&in_flight);
        let slug = slug.clone();
        tokio::spawn(async move {
            process_guarded(&state, &event, &in_flight, &slug).await;
        });
    }

    info!("Multi-repo event processor stopped");
}

/// Guard against concurrent processing of the same (repo, issue/PR number).
async fn process_guarded(state: &DaemonState, event: &WebhookEvent, in_flight: &InFlight, slug: &str) {
    if let Some(number) = event.number {
        let key = (slug.to_string(), number);
        {
            let mut set = in_flight.lock().await;
            if !set.insert(key.clone()) {
                warn!("Skipping event id={} for #{number} (already in-flight)", event.id);
                return;
            }
        }
        process_event(state, event).await;
        {
            let mut set = in_flight.lock().await;
            set.remove(&key);
        }
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

    // Force sync issues (bypass throttle — we know there's a new event)
    gh_sync::sync_issues_now(&state.gh, &state.db).await?;

    // Run triage pipeline
    let args = TriageArgs {
        issue: number,
        apply: state.apply,
        retriage: false,
    };

    pipelines::triage::run(&state.config, &state.db, &state.gh, &args, pipelines::triage::OutputFormat::Text, None).await?;

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

    // Force sync pulls (bypass throttle — we know there's a new event)
    gh_sync::sync_pulls_now(&state.gh, &state.db).await?;

    // Run PR analysis pipeline
    let args = PrArgs {
        pr: number,
        apply: state.apply,
    };

    pipelines::pr_analysis::run(&state.config, &state.db, &state.gh, &args, false, None).await?;

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
        &state.db,
        &state.gh,
        state.apply,
        Some(triggered_by),
    )
    .await?;

    // Post response as a comment
    if state.apply {
        state.gh.comment_issue(number, &response).await?;
        info!("Posted slash command response on #{number}");
    } else {
        info!("Dry-run slash command response for #{number}: {response}");
    }

    Ok(())
}
