//! GitHub polling fallback for when webhooks are not available.
//!
//! Polls /repos/{owner}/{repo}/events every N seconds and dispatches
//! new events to the processor queue, just like the webhook server would.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use super::processor::WebhookEvent;
use super::DaemonState;

/// Poll interval (default 30s, GitHub events API has 1-min cache)
const POLL_INTERVAL_SECS: u64 = 30;

/// `sync_log.table_name` slot reused to persist the poller's
/// `last_event_id` across daemon restarts. Without this, every restart
/// reads the last 30 GitHub events as fresh and re-runs the analyze /
/// triage pipelines, burning AI credits and posting duplicate review
/// comments when `apply=true`.
const POLLER_SYNC_KEY: &str = "__poller_last_event";

fn load_last_event_id(state: &DaemonState) -> Option<String> {
    state
        .db
        .get_sync_entry(POLLER_SYNC_KEY)
        .ok()
        .flatten()
        .and_then(|e| e.etag)
}

fn store_last_event_id(state: &DaemonState, id: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    if let Err(e) = state.db.update_sync_entry(POLLER_SYNC_KEY, &now, Some(id)) {
        warn!("Failed to persist poller last_event_id: {e}");
    }
}

/// Multi-repo poller: tags events with the repo slug before sending.
pub async fn run_multi(
    state: Arc<DaemonState>,
    tx: mpsc::Sender<(String, WebhookEvent)>,
    interval_secs: Option<u64>,
    slug: String,
) {
    let interval = Duration::from_secs(interval_secs.unwrap_or(POLL_INTERVAL_SECS));
    let mut last_event_id: Option<String> = load_last_event_id(&state);

    info!(
        "[{slug}] Event poller started (every {}s, last_event_id={:?})",
        interval.as_secs(),
        last_event_id
    );

    loop {
        tokio::time::sleep(interval).await;

        match poll_events(&state, &mut last_event_id).await {
            Ok(events) => {
                if events.is_empty() {
                    debug!("[{slug}] No new events");
                } else {
                    info!("[{slug}] Polled {} new event(s)", events.len());
                }
                for event in events {
                    if let Err(e) = tx.send((slug.clone(), event)).await {
                        error!("[{slug}] Failed to enqueue polled event: {e}");
                    }
                }
            }
            Err(e) => {
                warn!("[{slug}] Polling error: {e:#}");
            }
        }
    }
}

pub async fn run(
    state: Arc<DaemonState>,
    tx: mpsc::Sender<WebhookEvent>,
    interval_secs: Option<u64>,
) {
    let interval = Duration::from_secs(interval_secs.unwrap_or(POLL_INTERVAL_SECS));
    let mut last_event_id: Option<String> = load_last_event_id(&state);

    info!(
        "Event poller started (every {}s) — no webhook needed (last_event_id={:?})",
        interval.as_secs(),
        last_event_id
    );

    loop {
        tokio::time::sleep(interval).await;

        match poll_events(&state, &mut last_event_id).await {
            Ok(events) => {
                if events.is_empty() {
                    debug!("No new events");
                } else {
                    info!("Polled {} new event(s)", events.len());
                }
                for event in events {
                    if let Err(e) = tx.send(event).await {
                        error!("Failed to enqueue polled event: {e}");
                    }
                }
            }
            Err(e) => {
                warn!("Polling error: {e:#}");
            }
        }
    }
}

/// Fetch new events from GitHub Events API and return them as WebhookEvents.
async fn poll_events(
    state: &DaemonState,
    last_event_id: &mut Option<String>,
) -> anyhow::Result<Vec<WebhookEvent>> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/events?per_page=30",
        state.config.repo_owner, state.config.repo_name
    );

    // The GitHub events fetch is retried on transient transport failures
    // (premature connection EOF from a dropped keep-alive, 5xx, 429). Both
    // the GET and the body read share one retried closure so a mid-body
    // EOF re-issues the whole request.
    let (headers, body) = crate::retry::with_retry("poller: github events", || async {
        let response = state.gh().octocrab._get(&url).await?;
        let headers = response.headers().clone();
        let body = state.gh().octocrab.body_to_string(response).await?;
        Ok::<_, anyhow::Error>((headers, body))
    })
    .await?;

    // Surface rate-limit pressure. Headers we care about:
    // `x-ratelimit-remaining` (drops as we burn quota) and
    // `x-ratelimit-reset` (unix epoch seconds when it refills). Logging at
    // warn! when remaining < 100 lets the operator see they're about to
    // get throttled, well before it fires.
    if let Some(remaining) = headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u32>().ok())
    {
        if remaining < 100 {
            let reset = headers
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let reset_in = (reset - chrono::Utc::now().timestamp()).max(0);
            warn!(
                "GitHub rate limit running low: {remaining} requests remaining \
                 (resets in {reset_in}s). Consider increasing poll_interval."
            );
        }
    }

    let events = crate::github::parse_json_array(&body, "events")?;

    if events.is_empty() {
        return Ok(Vec::new());
    }

    // Find new events (everything after last_event_id)
    let mut new_events = Vec::new();
    for event in &events {
        let id = event.get("id").and_then(|v| v.as_str()).unwrap_or("");

        if let Some(ref last_id) = last_event_id {
            if id == last_id {
                break; // Reached last seen event
            }
        }

        new_events.push(event.clone());
    }

    // Update last seen — both in-memory and persisted to sync_log so
    // a daemon restart doesn't re-process the last 30 events.
    if let Some(first) = events.first() {
        if let Some(id) = first.get("id").and_then(|v| v.as_str()) {
            *last_event_id = Some(id.to_string());
            store_last_event_id(state, id);
        }
    }

    // Process in chronological order (API returns newest first)
    new_events.reverse();

    let mut result = Vec::new();
    for event in &new_events {
        let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

        let payload = event.get("payload").cloned().unwrap_or_default();
        let action = payload
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Map GitHub event types to webhook event types
        let (mapped_type, number) = match event_type {
            "IssuesEvent" if action == "opened" => {
                let n = payload
                    .get("issue")
                    .and_then(|i| i.get("number"))
                    .and_then(|n| n.as_u64());
                ("issues", n)
            }
            "PullRequestEvent" if action == "opened" || action == "synchronize" => {
                let n = payload
                    .get("pull_request")
                    .and_then(|p| p.get("number"))
                    .and_then(|n| n.as_u64());
                ("pull_request", n)
            }
            "IssueCommentEvent" if action == "created" => {
                let n = payload
                    .get("issue")
                    .and_then(|i| i.get("number"))
                    .and_then(|n| n.as_u64());
                // Only dispatch if it contains a slash command
                let body = payload
                    .get("comment")
                    .and_then(|c| c.get("body"))
                    .and_then(|b| b.as_str())
                    .unwrap_or("");
                if !body.contains(&state.config.branding.command_prefix) {
                    continue;
                }
                ("issue_comment", n)
            }
            _ => continue,
        };

        // Store in DB
        let payload_str = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to serialize poller event payload: {e}");
                continue;
            }
        };
        let event_id =
            match state
                .db
                .insert_webhook_event(mapped_type, &action, number, &payload_str)
            {
                Ok(id) => id,
                Err(e) => {
                    error!("Failed to store polled event: {e}");
                    continue;
                }
            };

        result.push(WebhookEvent {
            id: event_id,
            event_type: mapped_type.to_string(),
            action,
            number,
            payload: payload_str,
        });
    }

    Ok(result)
}
