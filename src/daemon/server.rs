use anyhow::Result;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::processor::WebhookEvent;
use super::DaemonState;

struct ServerState {
    daemon: Arc<DaemonState>,
    tx: mpsc::Sender<WebhookEvent>,
    secret: Option<String>,
}

pub async fn run(
    daemon: Arc<DaemonState>,
    tx: mpsc::Sender<WebhookEvent>,
    bind: &str,
    secret: Option<&str>,
) -> Result<()> {
    let state = Arc::new(ServerState {
        daemon,
        tx,
        secret: secret.map(|s| s.to_string()),
    });

    let app = Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/health", get(handle_health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    info!("Webhook server listening on {bind}");
    axum::serve(listener, app).await?;

    Ok(())
}

async fn handle_health(
    State(state): State<Arc<ServerState>>,
) -> impl IntoResponse {
    let pending = state.daemon.db.pending_event_count().unwrap_or(0);
    Json(json!({
        "status": "ok",
        "apply": state.daemon.apply,
        "pending_events": pending,
        "repo": format!("{}/{}", state.daemon.config.repo_owner, state.daemon.config.repo_name),
    }))
}

async fn handle_webhook(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // Validate HMAC signature if secret is configured
    if let Some(ref secret) = state.secret {
        let sig_header = headers
            .get("x-hub-signature-256")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !verify_signature(secret, &body, sig_header) {
            warn!("Invalid webhook signature");
            return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid signature"}))).into_response();
        }
    }

    // Parse event type from headers
    let event_type = match headers.get("x-github-event").and_then(|v| v.to_str().ok()) {
        Some(e) => e.to_string(),
        None => {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "missing x-github-event header"}))).into_response();
        }
    };

    // Parse payload
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to parse webhook payload: {e}");
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "invalid JSON"}))).into_response();
        }
    };

    let action = payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Filter: only process relevant events
    let should_process = match event_type.as_str() {
        "issues" => action == "opened",
        "pull_request" => action == "opened" || action == "synchronize",
        "issue_comment" => action == "created",
        _ => false,
    };

    if !should_process {
        info!("Ignoring event: {event_type}.{action}");
        return (StatusCode::OK, Json(json!({"status": "ignored", "event": event_type, "action": action}))).into_response();
    }

    // Extract number
    let number = match event_type.as_str() {
        "issues" => payload.get("issue").and_then(|i| i.get("number")).and_then(|n| n.as_u64()),
        "pull_request" => payload.get("pull_request").and_then(|p| p.get("number")).and_then(|n| n.as_u64()),
        "issue_comment" => payload.get("issue").and_then(|i| i.get("number")).and_then(|n| n.as_u64()),
        _ => None,
    };

    // Store in DB
    let payload_str = serde_json::to_string(&payload).unwrap_or_default();
    let event_id = match state.daemon.db.insert_webhook_event(
        &event_type,
        &action,
        number,
        &payload_str,
    ) {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("Failed to store webhook event: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "failed to store event"}))).into_response();
        }
    };

    info!("Received webhook: {event_type}.{action} number={number:?} id={event_id}");

    // Enqueue for processing
    let event = WebhookEvent {
        id: event_id,
        event_type: event_type.clone(),
        action: action.clone(),
        number,
        payload: payload_str,
    };

    if let Err(e) = state.tx.send(event).await {
        tracing::error!("Failed to enqueue event: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "queue full"}))).into_response();
    }

    (StatusCode::OK, Json(json!({"status": "accepted", "event": event_type, "action": action, "id": event_id}))).into_response()
}

fn verify_signature(secret: &str, body: &[u8], sig_header: &str) -> bool {
    let expected = match sig_header.strip_prefix("sha256=") {
        Some(h) => h,
        None => return false,
    };
    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let computed = hex::encode(mac.finalize().into_bytes());
    computed == expected
}
