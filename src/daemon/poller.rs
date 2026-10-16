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
        if state.sleep_or_cancel(interval).await {
            info!("[{slug}] repo removed — stopping poller");
            break;
        }

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
    let gh = state.gh();
    let Some(client) = gh.as_github() else {
        // GitLab / Gitea / Forgejo / Azure DevOps have no GitHub-style
        // repository events feed: diff the forge state instead.
        return poll_forge_changes(state, last_event_id).await;
    };

    let url = format!(
        "https://api.github.com/repos/{}/{}/events?per_page=30",
        state.config.repo_owner, state.config.repo_name
    );

    // The GitHub events fetch is retried on transient transport failures
    // (premature connection EOF from a dropped keep-alive, 5xx, 429). Both
    // the GET and the body read share one retried closure so a mid-body
    // EOF re-issues the whole request.
    let (headers, body) = crate::retry::with_retry("poller: github events", || async {
        let response = client.octocrab._get(&url).await?;
        let headers = response.headers().clone();
        let body = client.octocrab.body_to_string(response).await?;
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

/// Last head SHA seen by the generic poller, per (repo, PR), so a push is
/// detected even when the periodic sync already refreshed the DB cache.
static SEEN_HEADS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<(String, u64), String>>,
> = std::sync::LazyLock::new(Default::default);

/// Cursor prefix distinguishing the generic poller's timestamp from a
/// GitHub event id in the shared sync-log slot.
const FORGE_CURSOR_PREFIX: &str = "ts:";

/// Polling for forges without an events API: everything created/updated in
/// the window `(cursor, poll start]` becomes the same events a webhook would
/// produce — `issues.opened`, `pull_request.opened` / `.synchronize` (head
/// SHA changed) and, where the provider supports it, `issue_comment.created`
/// with a GitHub-shaped payload for the slash-command handler.
///
/// The first poll only records the cursor: the backlog is handled by the
/// scheduler's sync + triage, not replayed as events.
async fn poll_forge_changes(
    state: &DaemonState,
    cursor: &mut Option<String>,
) -> anyhow::Result<Vec<WebhookEvent>> {
    use crate::git_provider::Capability;

    let gh = state.gh();
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let since = match cursor
        .as_deref()
        .and_then(|c| c.strip_prefix(FORGE_CURSOR_PREFIX))
    {
        Some(ts) => ts.to_string(),
        None => {
            let c = format!("{FORGE_CURSOR_PREFIX}{now}");
            store_last_event_id(state, &c);
            *cursor = Some(c);
            return Ok(Vec::new());
        }
    };
    let in_window = |ts: &str| {
        let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
        match (parse(ts), parse(&since), parse(&now)) {
            (Some(t), Some(lo), Some(hi)) => t > lo && t <= hi,
            _ => false,
        }
    };
    let slug = state.config.repo_slug();
    let mut raw: Vec<(&'static str, &'static str, u64, serde_json::Value)> = Vec::new();

    for issue in gh.fetch_all_issues(Some(&since)).await? {
        if issue.state == "open" && in_window(&issue.created_at) {
            raw.push((
                "issues",
                "opened",
                issue.number,
                serde_json::json!({"action": "opened", "issue": {"number": issue.number}}),
            ));
        }
    }

    for pr in gh.fetch_pulls_incremental("open", Some(&since)).await? {
        let head = pr.head_sha.clone().unwrap_or_default();
        let key = (slug.clone(), pr.number);
        let previous = SEEN_HEADS
            .lock()
            .ok()
            .and_then(|m| m.get(&key).cloned())
            .or_else(|| {
                state
                    .db
                    .get_pull(pr.number)
                    .ok()
                    .flatten()
                    .and_then(|p| p.head_sha)
            });
        if let Ok(mut m) = SEEN_HEADS.lock() {
            m.insert(key, head.clone());
        }
        let action = if in_window(&pr.created_at) {
            "opened"
        } else if in_window(&pr.updated_at)
            && !head.is_empty()
            && previous.as_deref().is_some_and(|p| p != head)
        {
            "synchronize"
        } else {
            continue;
        };
        raw.push((
            "pull_request",
            action,
            pr.number,
            serde_json::json!({"action": action, "pull_request": {"number": pr.number}}),
        ));
    }

    if gh.supports(Capability::CommentPolling) {
        let prefix = &state.config.branding.command_prefix;
        let marker = state.config.branding.comment_marker();
        for c in gh.fetch_comments_since(&since).await? {
            if !in_window(&c.created_at)
                || !c.body.contains(prefix.as_str())
                || c.body.contains(&marker)
            {
                continue;
            }
            let mut issue = serde_json::json!({"number": c.number});
            if c.is_pr {
                issue["pull_request"] = serde_json::json!({});
            }
            raw.push((
                "issue_comment",
                "created",
                c.number,
                serde_json::json!({
                    "action": "created",
                    "issue": issue,
                    "comment": {"id": c.id, "body": c.body, "user": {"login": c.author}},
                    "sender": {"login": c.author},
                }),
            ));
        }
    }

    let c = format!("{FORGE_CURSOR_PREFIX}{now}");
    store_last_event_id(state, &c);
    *cursor = Some(c);

    let mut result = Vec::with_capacity(raw.len());
    for (event_type, action, number, payload) in raw {
        let payload_str = payload.to_string();
        match state
            .db
            .insert_webhook_event(event_type, action, Some(number), &payload_str)
        {
            Ok(id) => result.push(WebhookEvent {
                id,
                event_type: event_type.to_string(),
                action: action.to_string(),
                number: Some(number),
                payload: payload_str,
            }),
            Err(e) => error!("Failed to store polled event: {e}"),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_provider::gitea::GiteaProvider;
    use crate::git_provider::http::{Auth, Rest};
    use crate::git_provider::mock::{MockServer, Reply};
    use serde_json::json;

    const R: &str = "/api/v1/repos/acme/widgets";

    fn pr(number: u64, head: &str, created: &str, updated: &str) -> serde_json::Value {
        json!({"number": number, "title": "t", "state": "open", "head": {"sha": head, "ref": "f"},
               "base": {"sha": "b", "ref": "main"}, "created_at": created, "updated_at": updated})
    }

    #[tokio::test]
    async fn generic_poller_synthesizes_webhook_events() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/issues"),
            Reply::json(json!([
                {"number": 1, "title": "new", "state": "open",
                 "created_at": "2026-05-01T10:05:00Z", "updated_at": "2026-05-01T10:05:00Z"},
                {"number": 2, "title": "old", "state": "open",
                 "created_at": "2026-04-01T00:00:00Z", "updated_at": "2026-05-01T10:06:00Z"}
            ])),
        );
        server.on(
            "GET",
            &format!("{R}/pulls"),
            Reply::json(json!([
                pr(3, "h3", "2026-05-01T10:06:00Z", "2026-05-01T10:06:00Z"),
                pr(4, "new", "2026-04-01T00:00:00Z", "2026-05-01T10:07:00Z"),
                pr(5, "same", "2026-04-01T00:00:00Z", "2026-05-01T10:07:00Z")
            ])),
        );
        server.on(
            "GET",
            &format!("{R}/issues/comments"),
            Reply::json(json!([
                {"id": 7, "body": "/wshm triage", "user": {"login": "bob"},
                 "created_at": "2026-05-01T10:08:00Z",
                 "issue_url": "https://h/acme/widgets/issues/2", "pull_request_url": ""},
                {"id": 8, "body": "/wshm help <!-- wshm -->", "user": {"login": "bot"},
                 "created_at": "2026-05-01T10:08:00Z",
                 "issue_url": "https://h/acme/widgets/issues/2", "pull_request_url": ""}
            ])),
        );
        let config = crate::config::Config {
            git_provider: Some("gitea".into()),
            repo_owner: "acme".into(),
            repo_name: "widgets".into(),
            ..Default::default()
        };
        let gh = Arc::new(GiteaProvider::with_rest(
            Rest::new("Gitea", Auth::None).unwrap(),
            &server.url(),
            "acme",
            "widgets",
            config.branding.comment_marker(),
            "gitea",
        ));
        let db = Arc::new(crate::Database::open_memory().unwrap());
        for (n, head) in [(4, "old"), (5, "same")] {
            let mut p = crate::git_provider::GitProvider::fetch_pulls(gh.as_ref())
                .await
                .unwrap()
                .into_iter()
                .find(|p| p.number == n)
                .unwrap();
            p.head_sha = Some(head.into());
            db.upsert_pull(&p).unwrap();
        }
        let state = DaemonState::new(db, gh, Arc::new(config), false);

        // First poll only records a cursor.
        let mut cursor = None;
        assert!(poll_events(&state, &mut cursor).await.unwrap().is_empty());
        assert!(cursor.as_deref().unwrap().starts_with(FORGE_CURSOR_PREFIX));

        let mut cursor = Some(format!("{FORGE_CURSOR_PREFIX}2026-05-01T10:00:00Z"));
        let events = poll_events(&state, &mut cursor).await.unwrap();
        let summary: Vec<(String, String, Option<u64>)> = events
            .iter()
            .map(|e| (e.event_type.clone(), e.action.clone(), e.number))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("issues".into(), "opened".into(), Some(1)),
                ("pull_request".into(), "opened".into(), Some(3)),
                ("pull_request".into(), "synchronize".into(), Some(4)),
                ("issue_comment".into(), "created".into(), Some(2)),
            ]
        );
        let comment: serde_json::Value = serde_json::from_str(&events[3].payload).unwrap();
        assert_eq!(comment["comment"]["user"]["login"], "bob");
        assert!(comment["issue"].get("pull_request").is_none());
        assert_ne!(cursor.as_deref(), Some("ts:2026-05-01T10:00:00Z"));

        // Same head again: no second synchronize.
        let mut cursor = Some(format!("{FORGE_CURSOR_PREFIX}2026-05-01T10:00:00Z"));
        let again = poll_events(&state, &mut cursor).await.unwrap();
        assert!(!again.iter().any(|e| e.action == "synchronize"));
        assert!(state
            .db
            .as_ref()
            .get_sync_entry(POLLER_SYNC_KEY)
            .unwrap()
            .is_some());
    }
}
