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

pub async fn run(
    state: Arc<DaemonState>,
    tx: mpsc::Sender<WebhookEvent>,
    interval_secs: Option<u64>,
) {
    let interval = Duration::from_secs(interval_secs.unwrap_or(POLL_INTERVAL_SECS));
    let mut last_event_id: Option<String> = None;

    info!(
        "Event poller started (every {}s) — no webhook needed",
        interval.as_secs()
    );

    loop {
        tokio::time::sleep(interval).await;

        match poll_events(&state, &tx, &mut last_event_id).await {
            Ok(count) => {
                if count > 0 {
                    info!("Polled {count} new event(s)");
                } else {
                    debug!("No new events");
                }
            }
            Err(e) => {
                warn!("Polling error: {e:#}");
            }
        }
    }
}

async fn poll_events(
    state: &DaemonState,
    tx: &mpsc::Sender<WebhookEvent>,
    last_event_id: &mut Option<String>,
) -> anyhow::Result<usize> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/events?per_page=30",
        state.config.repo_owner, state.config.repo_name
    );

    let response = state
        .gh
        .octocrab
        ._get(&url)
        .await?;

    let body = state.gh.octocrab.body_to_string(response).await?;
    let events: Vec<serde_json::Value> = serde_json::from_str(&body)?;

    if events.is_empty() {
        return Ok(0);
    }

    // Find new events (everything after last_event_id)
    let mut new_events = Vec::new();
    for event in &events {
        let id = event
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if let Some(ref last_id) = last_event_id {
            if id == last_id {
                break; // Reached last seen event
            }
        }

        new_events.push(event.clone());
    }

    // Update last seen
    if let Some(first) = events.first() {
        if let Some(id) = first.get("id").and_then(|v| v.as_str()) {
            *last_event_id = Some(id.to_string());
        }
    }

    // Process in chronological order (API returns newest first)
    new_events.reverse();

    let mut dispatched = 0;
    for event in &new_events {
        let event_type = event
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let payload = event.get("payload").cloned().unwrap_or_default();
        let action = payload
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Map GitHub event types to webhook event types
        let (mapped_type, number) = match event_type {
            "IssuesEvent" if action == "opened" => {
                let n = payload.get("issue").and_then(|i| i.get("number")).and_then(|n| n.as_u64());
                ("issues", n)
            }
            "PullRequestEvent" if action == "opened" || action == "synchronize" => {
                let n = payload.get("pull_request").and_then(|p| p.get("number")).and_then(|n| n.as_u64());
                ("pull_request", n)
            }
            "IssueCommentEvent" if action == "created" => {
                let n = payload.get("issue").and_then(|i| i.get("number")).and_then(|n| n.as_u64());
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
        let payload_str = serde_json::to_string(&payload).unwrap_or_default();
        let event_id = match state.db.insert_webhook_event(
            mapped_type,
            &action,
            number,
            &payload_str,
        ) {
            Ok(id) => id,
            Err(e) => {
                error!("Failed to store polled event: {e}");
                continue;
            }
        };

        let webhook_event = WebhookEvent {
            id: event_id,
            event_type: mapped_type.to_string(),
            action,
            number,
            payload: payload_str,
        };

        if let Err(e) = tx.send(webhook_event).await {
            error!("Failed to enqueue polled event: {e}");
        } else {
            dispatched += 1;
        }
    }

    Ok(dispatched)
}
