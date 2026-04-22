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
use super::{DaemonState, MultiDaemonState};

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
    tls: Option<(String, String)>,
) -> Result<()> {
    if secret.is_none() {
        warn!("No webhook secret configured — webhook endpoint is unauthenticated! Set WSHM_WEBHOOK_SECRET or [daemon].webhook_secret");
    }

    let slug = daemon.config.repo_slug();

    let state = Arc::new(ServerState {
        daemon: Arc::clone(&daemon),
        tx,
        secret: secret.map(|s| s.to_string()),
    });

    let webhook_routes = Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/health", get(handle_health))
        .with_state(state);

    // Build a single-repo MultiDaemonState for the web UI
    let mut repos = std::collections::HashMap::new();
    repos.insert(slug, daemon);
    let multi = Arc::new(super::MultiDaemonState { repos });
    // OSS: no Pro extensions
    let web = super::web::web_routes_with_extensions(multi, None, None);

    let app = webhook_routes.merge(web);

    if let Some((cert_path, key_path)) = tls {
        // crypto provider already installed in run_multi/run
        let tls_config =
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path).await?;
        let addr: std::net::SocketAddr = bind.parse()?;
        info!("Webhook server listening on https://{bind} (TLS enabled)");
        axum_server::bind_rustls(addr, tls_config)
            .serve(app.into_make_service())
            .await?;
    } else {
        let listener = tokio::net::TcpListener::bind(bind).await?;
        info!("Webhook server listening on http://{bind} (web UI enabled)");
        axum::serve(listener, app).await?;
    }

    Ok(())
}

async fn handle_health(State(state): State<Arc<ServerState>>) -> impl IntoResponse {
    let pending = state.daemon.db.pending_event_count().unwrap_or_else(|e| {
        tracing::warn!("Failed to query pending events: {e}");
        0
    });
    Json(json!({
        "status": "ok",
        "apply": state.daemon.apply,
        "pending_events": pending,
        "repo": format!("{}/{}", state.daemon.config.repo_owner, state.daemon.config.repo_name),
    }))
}

/// Maximum webhook payload size (25 MB — GitHub's own limit)
const MAX_WEBHOOK_SIZE: usize = 25 * 1024 * 1024;

async fn handle_webhook(
    State(state): State<Arc<ServerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let (event_type, action, payload) =
        match validate_webhook(state.secret.as_deref(), &headers, &body) {
            Ok(v) => v,
            Err(resp) => return resp,
        };

    // Filter: only process relevant events
    if !should_process_event(&event_type, &action) {
        info!("Ignoring event: {event_type}.{action}");
        return (
            StatusCode::OK,
            Json(json!({"status": "ignored", "event": event_type, "action": action})),
        )
            .into_response();
    }

    // Extract number
    let number = extract_number(&event_type, &payload);

    // Store in DB
    let payload_str = match serde_json::to_string(&payload) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize webhook payload: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to serialize payload"})),
            )
                .into_response();
        }
    };
    let event_id =
        match state
            .daemon
            .db
            .insert_webhook_event(&event_type, &action, number, &payload_str)
        {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("Failed to store webhook event: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "failed to store event"})),
                )
                    .into_response();
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
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "queue full"})),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({"status": "accepted", "event": event_type, "action": action, "id": event_id})),
    )
        .into_response()
}

// ── Multi-repo server ──────────────────────────────────────────

struct MultiServerState {
    multi: Arc<MultiDaemonState>,
    tx: mpsc::Sender<(String, WebhookEvent)>,
    secret: Option<String>,
}

pub async fn run_multi(
    multi: Arc<MultiDaemonState>,
    tx: mpsc::Sender<(String, WebhookEvent)>,
    bind: &str,
    secret: Option<&str>,
    tls: Option<(String, String)>,
) -> Result<()> {
    let state = Arc::new(MultiServerState {
        multi: Arc::clone(&multi),
        tx,
        secret: secret.map(|s| s.to_string()),
    });

    // Webhook + health routes (original)
    let webhook_routes = Router::new()
        .route("/webhook", post(handle_webhook_multi))
        .route("/health", get(handle_health_multi))
        .with_state(state);

    // Web UI + API routes (Svelte SPA + /api/v1/*)
    // OSS: no Pro extensions
    let web = super::web::web_routes_with_extensions(multi, None, None);

    let app = webhook_routes.merge(web);

    if let Some((cert_path, key_path)) = tls {
        // crypto provider already installed in run_multi/run
        let tls_config =
            axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path).await?;
        let addr: std::net::SocketAddr = bind.parse()?;
        info!("Multi-repo server listening on https://{bind} (TLS enabled)");
        axum_server::bind_rustls(addr, tls_config)
            .serve(app.into_make_service())
            .await?;
    } else {
        let listener = tokio::net::TcpListener::bind(bind).await?;
        info!("Multi-repo webhook server listening on http://{bind} (web UI enabled)");
        axum::serve(listener, app).await?;
    }

    Ok(())
}

async fn handle_health_multi(State(state): State<Arc<MultiServerState>>) -> impl IntoResponse {
    let repos: Vec<Value> = state
        .multi
        .repos
        .iter()
        .map(|(slug, ds)| {
            let pending = ds.db.pending_event_count().unwrap_or_else(|e| {
                tracing::warn!("[{slug}] Failed to query pending events: {e}");
                0
            });
            json!({
                "repo": slug,
                "apply": ds.apply,
                "pending_events": pending,
            })
        })
        .collect();

    Json(json!({
        "status": "ok",
        "repos": repos,
    }))
}

async fn handle_webhook_multi(
    State(state): State<Arc<MultiServerState>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let (event_type, action, payload) =
        match validate_webhook(state.secret.as_deref(), &headers, &body) {
            Ok(v) => v,
            Err(resp) => return resp,
        };

    // Route by repository.full_name
    let repo_slug = match payload
        .get("repository")
        .and_then(|r| r.get("full_name"))
        .and_then(|n| n.as_str())
    {
        Some(s) => s.to_string(),
        None => {
            warn!("Webhook payload missing repository.full_name");
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing repository.full_name"})),
            )
                .into_response();
        }
    };

    let daemon = match state.multi.repos.get(&repo_slug) {
        Some(d) => d,
        None => {
            info!("Ignoring webhook for unconfigured repo: {repo_slug}");
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "repo not configured", "repo": repo_slug})),
            )
                .into_response();
        }
    };

    // Filter relevant events
    if !should_process_event(&event_type, &action) {
        return (
            StatusCode::OK,
            Json(json!({"status": "ignored", "repo": repo_slug, "event": event_type})),
        )
            .into_response();
    }

    let number = extract_number(&event_type, &payload);

    // Store in the correct repo's DB
    let payload_str = match serde_json::to_string(&payload) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("Failed to serialize webhook payload: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to serialize payload"})),
            )
                .into_response();
        }
    };
    let event_id = match daemon
        .db
        .insert_webhook_event(&event_type, &action, number, &payload_str)
    {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("[{repo_slug}] Failed to store webhook event: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to store event"})),
            )
                .into_response();
        }
    };

    info!("[{repo_slug}] Received webhook: {event_type}.{action} number={number:?} id={event_id}");

    let event = WebhookEvent {
        id: event_id,
        event_type: event_type.clone(),
        action: action.clone(),
        number,
        payload: payload_str,
    };

    if let Err(e) = state.tx.send((repo_slug.clone(), event)).await {
        tracing::error!("[{repo_slug}] Failed to enqueue event: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "queue full"})),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(json!({"status": "accepted", "repo": repo_slug, "event": event_type, "id": event_id})),
    )
        .into_response()
}

// ── Shared helpers ─────────────────────────────────────────────

/// Common webhook validation: size, signature, event type, payload parsing.
/// Returns (event_type, action, payload) on success, or an error response.
#[allow(clippy::result_large_err)]
fn validate_webhook(
    secret: Option<&str>,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(String, String, Value), axum::response::Response> {
    // Reject oversized payloads
    if body.len() > MAX_WEBHOOK_SIZE {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"error": "payload too large"})),
        )
            .into_response());
    }

    // Validate HMAC signature if secret is configured
    if let Some(secret) = secret {
        let sig_header = headers
            .get("x-hub-signature-256")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !verify_signature(secret, body, sig_header) {
            warn!("Invalid webhook signature");
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid signature"})),
            )
                .into_response());
        }
    }

    // Parse event type from headers
    let event_type = match headers.get("x-github-event").and_then(|v| v.to_str().ok()) {
        Some(e) => e.to_string(),
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing x-github-event header"})),
            )
                .into_response());
        }
    };

    // Parse payload (serde_json enforces a default recursion limit of 128 levels)
    let payload: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to parse webhook payload: {e}");
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "invalid JSON"})),
            )
                .into_response());
        }
    };

    let action = payload
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok((event_type, action, payload))
}

fn should_process_event(event_type: &str, action: &str) -> bool {
    match event_type {
        "issues" => action == "opened",
        "pull_request" => action == "opened" || action == "synchronize",
        "issue_comment" => action == "created",
        _ => false,
    }
}

fn extract_number(event_type: &str, payload: &Value) -> Option<u64> {
    match event_type {
        "issues" => payload
            .get("issue")
            .and_then(|i| i.get("number"))
            .and_then(|n| n.as_u64()),
        "pull_request" => payload
            .get("pull_request")
            .and_then(|p| p.get("number"))
            .and_then(|n| n.as_u64()),
        "issue_comment" => payload
            .get("issue")
            .and_then(|i| i.get("number"))
            .and_then(|n| n.as_u64()),
        _ => None,
    }
}

fn verify_signature(secret: &str, body: &[u8], sig_header: &str) -> bool {
    let expected_hex = match sig_header.strip_prefix("sha256=") {
        Some(h) => h,
        None => return false,
    };
    let expected_bytes = match hex::decode(expected_hex) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    // Constant-time comparison via hmac::Mac::verify_slice
    mac.verify_slice(&expected_bytes).is_ok()
}
