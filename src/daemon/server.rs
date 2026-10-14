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
        warn!("No webhook secret configured — /webhook will reject deliveries with 503. Set WSHM_WEBHOOK_SECRET or [daemon].webhook_secret (or WSHM_ALLOW_UNSIGNED_WEBHOOKS=1 to opt in to an unauthenticated endpoint).");
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
        // Raise the body limit from axum's implicit 2MB default to the
        // 25MB GitHub maximum so legitimate large deliveries reach the
        // explicit size check in validate_webhook instead of being
        // silently rejected with a 413.
        .layer(axum::extract::DefaultBodyLimit::max(MAX_WEBHOOK_SIZE))
        .with_state(state);

    // Build a single-repo MultiDaemonState for the web UI (no dynamic
    // runtime — add_repo unavailable in mono-repo mode).
    let mut repos = std::collections::HashMap::new();
    repos.insert(slug, daemon);
    let multi = Arc::new(super::MultiDaemonState::new(repos));

    // Open a default UserStore on ~/.wshm/users.db so the OSS single-repo
    // daemon also has a working RBAC + login flow out of the box. Same logic
    // as run_multi_with_extensions; kept here too because daemon::run takes
    // a different code path.
    let users: Option<Arc<dyn crate::auth::UserStoreBackend>> = match crate::auth::UserStore::open(
        &dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".wshm")
            .join("users.db"),
    ) {
        Ok(store) => Some(Arc::new(store) as Arc<dyn crate::auth::UserStoreBackend>),
        Err(e) => {
            warn!("Failed to open users db: {e} — login disabled");
            None
        }
    };
    if let Some(store) = users.as_ref() {
        if let Err(e) = crate::auth::seed_admin_if_empty(store.as_ref()).await {
            warn!("Failed to seed admin user: {e}");
        }
    }
    let logs = super::log_buffer::global();
    let web = super::web::web_routes_with_extensions(multi, users, logs, None, None, None);

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

    // Replay protection: dedupe on the forge delivery UUID within REPLAY_TTL.
    // GitHub sends it in x-github-delivery; Gitea/Forgejo (used by wshm-pro)
    // send it in x-gitea-delivery / x-forgejo-delivery. Consult all three so
    // dedup is not silently bypassed on a non-GitHub forge. Without this, a
    // captured signed payload verifies forever and can be replayed to
    // re-trigger issue/PR processing (auto-fix runs, comment posting, LLM spend).
    let delivery_id = headers
        .get("x-github-delivery")
        .or_else(|| headers.get("x-gitea-delivery"))
        .or_else(|| headers.get("x-forgejo-delivery"))
        .or_else(|| headers.get("x-gitlab-event-uuid"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // When a webhook secret is configured the HMAC already proved authenticity,
    // so a delivery with no id is almost certainly a forged/malformed replay:
    // reject it rather than letting it bypass dedup. Without a secret we keep
    // the lenient behaviour for older senders that omit the header.
    if delivery_id.is_empty() && state.secret.is_some() {
        warn!("Rejecting webhook with no delivery id while a secret is configured");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing delivery id"})),
        )
            .into_response();
    }
    if check_replay(delivery_id).is_err() {
        info!("Duplicate webhook delivery ignored: {delivery_id}");
        return (
            StatusCode::OK,
            Json(json!({"status": "duplicate", "delivery_id": delivery_id})),
        )
            .into_response();
    }

    let config = &state.daemon.config;
    if !format_matches_provider(&headers, config) {
        return provider_mismatch(&config.repo_slug());
    }
    if headers.contains_key("x-gitlab-event")
        && payload["repository"]["full_name"].as_str() != Some(config.repo_slug().as_str())
    {
        return provider_mismatch(&config.repo_slug());
    }

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
    extensions: super::DaemonExtensions,
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
        // See run(): raise the body limit to the 25MB GitHub maximum so the
        // explicit size check in validate_webhook is the enforced bound.
        .layer(axum::extract::DefaultBodyLimit::max(MAX_WEBHOOK_SIZE))
        .with_state(state);

    // Seed admin user if a UserStore is provided and the table is empty.
    if let Some(store) = extensions.users.as_ref() {
        if let Err(e) = crate::auth::seed_admin_if_empty(store.as_ref()).await {
            warn!("Failed to seed admin user: {e}");
        }
    }

    // Web UI + API routes (Svelte SPA + /api/v1/*).
    let web = super::web::web_routes_with_extensions(
        multi,
        extensions.users,
        extensions.logs,
        extensions.secrets,
        extensions.extra_api,
        extensions.spa_override,
    );

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
    // Snapshot the repo map (cloning the Arc<DaemonState> handles) and drop the
    // read guard BEFORE running the per-repo blocking DB queries. Holding the
    // guard across a slow/contended pending_event_count() would block
    // repos.write() (dynamic repo add/remove, config reload) for the whole scan.
    let snapshot: Vec<(String, Arc<DaemonState>)> = {
        let repos_guard = state.multi.repos.read().await;
        repos_guard
            .iter()
            .map(|(slug, ds)| (slug.clone(), Arc::clone(ds)))
            .collect()
    };
    let repos: Vec<Value> = snapshot
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

/// In-memory replay cache for inbound webhooks. Keyed on the forge delivery
/// UUID (`x-github-delivery` / `x-gitea-delivery` / `x-forgejo-delivery`);
/// values are the timestamp the delivery was
/// first seen. Entries older than `REPLAY_TTL` are pruned lazily and the
/// map is capped at `REPLAY_MAX` to bound memory.
///
/// Replay protection complements HMAC signature verification: a captured
/// signed payload would still verify forever; this dedupes deliveries
/// that arrive twice, whether through GitHub's documented retries or a
/// malicious replay.
static REPLAY_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = std::sync::OnceLock::new();
const REPLAY_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
const REPLAY_MAX: usize = 10_000;

/// Returns Ok(()) when the delivery is fresh, Err(()) when it's a
/// duplicate. An empty delivery_id is treated as fresh here (the caller
/// already rejects empty ids when a secret is configured; older
/// secret-less senders may legitimately omit the header).
fn check_replay(delivery_id: &str) -> Result<(), ()> {
    if delivery_id.is_empty() {
        return Ok(());
    }
    let cache =
        REPLAY_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    // Recover from a poisoned guard rather than failing open: a panic in a
    // prior holder must not permanently disable replay dedup. The cache is a
    // plain map of delivery-id -> timestamp, so a partially-updated map is
    // still safe to keep using.
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    if let Some(seen_at) = map.get(delivery_id) {
        if now.duration_since(*seen_at) < REPLAY_TTL {
            return Err(());
        }
    }
    if map.len() >= REPLAY_MAX {
        map.retain(|_, t| now.duration_since(*t) < REPLAY_TTL);
        if map.len() >= REPLAY_MAX {
            // Still full after pruning — drop a quarter at random.
            let drop_n = REPLAY_MAX / 4;
            let to_drop: Vec<String> = map.keys().take(drop_n).cloned().collect();
            for k in to_drop {
                map.remove(&k);
            }
        }
    }
    map.insert(delivery_id.to_string(), now);
    Ok(())
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

    // Replay protection: dedupe on the forge delivery UUID within REPLAY_TTL.
    // GitHub sends it in x-github-delivery; Gitea/Forgejo (used by wshm-pro)
    // send it in x-gitea-delivery / x-forgejo-delivery. Consult all three so
    // dedup is not silently bypassed on a non-GitHub forge.
    let delivery_id = headers
        .get("x-github-delivery")
        .or_else(|| headers.get("x-gitea-delivery"))
        .or_else(|| headers.get("x-forgejo-delivery"))
        .or_else(|| headers.get("x-gitlab-event-uuid"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // When a webhook secret is configured the HMAC already proved authenticity,
    // so a delivery with no id is almost certainly a forged/malformed replay:
    // reject it rather than letting it bypass dedup. Without a secret we keep
    // the lenient behaviour for older senders that omit the header.
    if delivery_id.is_empty() && state.secret.is_some() {
        warn!("Rejecting webhook with no delivery id while a secret is configured");
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing delivery id"})),
        )
            .into_response();
    }
    if check_replay(delivery_id).is_err() {
        info!("Duplicate webhook delivery ignored: {delivery_id}");
        return (
            StatusCode::OK,
            Json(json!({"status": "duplicate", "delivery_id": delivery_id})),
        )
            .into_response();
    }

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

    let repos_guard = state.multi.repos.read().await;
    let daemon = match repos_guard.get(&repo_slug) {
        Some(d) => Arc::clone(d),
        None => {
            info!("Ignoring webhook for unconfigured repo: {repo_slug}");
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "repo not configured", "repo": repo_slug})),
            )
                .into_response();
        }
    };
    drop(repos_guard);

    // Routing used a sender-supplied name: the delivery format must also be
    // the one that repo's forge actually emits.
    if !format_matches_provider(&headers, &daemon.config) {
        return provider_mismatch(&repo_slug);
    }

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

    // GitLab authenticates with a shared token header instead of an HMAC
    // and has its own event names/payloads: translated to the GitHub shapes
    // the rest of the pipeline expects.
    if let Some(gl_event) = headers.get("x-gitlab-event").and_then(|v| v.to_str().ok()) {
        return validate_gitlab_webhook(gl_event, headers, body);
    }

    // Validate HMAC signature. Without a configured secret the endpoint
    // cannot authenticate the sender, so we refuse to process the delivery
    // rather than silently accepting forged issues/PRs/comments into the
    // pipeline. Operators who genuinely want an unauthenticated endpoint
    // must opt in explicitly with WSHM_ALLOW_UNSIGNED_WEBHOOKS=1.
    match secret {
        Some(secret) => {
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
        None => {
            let allow_unsigned = std::env::var("WSHM_ALLOW_UNSIGNED_WEBHOOKS")
                .ok()
                .filter(|v| v == "1" || v == "true")
                .is_some();
            if !allow_unsigned {
                warn!(
                    "Rejecting webhook: no secret configured. Set WSHM_WEBHOOK_SECRET / \
                     [daemon].webhook_secret, or WSHM_ALLOW_UNSIGNED_WEBHOOKS=1 to opt in \
                     to an unauthenticated endpoint."
                );
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "webhook secret not configured"})),
                )
                    .into_response());
            }
            warn!("Processing UNSIGNED webhook (WSHM_ALLOW_UNSIGNED_WEBHOOKS opt-in active)");
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
    // Gitea/Forgejo send GitHub-compatible headers and payloads, but name a
    // PR push `synchronized`.
    let action = if action == "synchronized" {
        "synchronize".to_string()
    } else {
        action
    };

    Ok((event_type, action, payload))
}

/// Validate and translate a GitLab webhook (`X-Gitlab-Event`,
/// `X-Gitlab-Token`): https://docs.gitlab.com/user/project/integrations/webhooks/
#[allow(clippy::result_large_err)]
fn validate_gitlab_webhook(
    gl_event: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Result<(String, String, Value), axum::response::Response> {
    let payload: Value = serde_json::from_slice(body).map_err(|e| {
        warn!("Failed to parse GitLab webhook payload: {e}");
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid JSON"})),
        )
            .into_response()
    })?;
    // GitLab's token is sent in clear to (and visible to Maintainers of) the
    // GitLab project, so it must never be the GitHub HMAC key: it has its
    // own secret, per repo (secret store `gitlab_webhook_token`, repo scope
    // then global) or WSHM_GITLAB_WEBHOOK_TOKEN.
    let path = payload["project"]["path_with_namespace"].as_str();
    let expected = crate::secrets::resolve(
        crate::secrets::global().as_ref(),
        path,
        GITLAB_WEBHOOK_TOKEN_KEY,
        "WSHM_GITLAB_WEBHOOK_TOKEN",
    )
    .filter(|t| !t.trim().is_empty());
    match expected {
        Some(expected) => {
            let token = headers
                .get("x-gitlab-token")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !constant_time_eq(&expected, token) {
                warn!("Invalid GitLab webhook token");
                return Err((
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "invalid token"})),
                )
                    .into_response());
            }
        }
        None => {
            let allow_unsigned = std::env::var("WSHM_ALLOW_UNSIGNED_WEBHOOKS")
                .ok()
                .filter(|v| v == "1" || v == "true")
                .is_some();
            if !allow_unsigned {
                warn!(
                    "Rejecting GitLab webhook: no GitLab token configured (secret \
                     `{GITLAB_WEBHOOK_TOKEN_KEY}` or WSHM_GITLAB_WEBHOOK_TOKEN)"
                );
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": "gitlab webhook token not configured"})),
                )
                    .into_response());
            }
        }
    }
    Ok(translate_gitlab(gl_event, &payload)
        .unwrap_or_else(|| (format!("gitlab:{gl_event}"), String::new(), payload)))
}

/// Secret-store key of the GitLab webhook token (distinct from the GitHub
/// HMAC secret).
pub const GITLAB_WEBHOOK_TOKEN_KEY: &str = "gitlab_webhook_token";

/// A delivery's wire format must match the target repo's forge: GitLab
/// format only for gitlab repos, GitHub format (GitHub, Gitea, Forgejo)
/// only for those. Azure DevOps webhooks are not supported. Stops a sender
/// of one forge from driving a repo hosted on another.
fn format_matches_provider(headers: &HeaderMap, config: &crate::config::Config) -> bool {
    let gitlab_format = headers.contains_key("x-gitlab-event");
    match crate::git_provider::provider_id(config) {
        "gitlab" => gitlab_format,
        "github" | "gitea" | "forgejo" => !gitlab_format,
        _ => false,
    }
}

fn provider_mismatch(repo: &str) -> axum::response::Response {
    warn!("Rejecting webhook for {repo}: delivery format does not match the repo's git_provider");
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error": "webhook format does not match repo provider", "repo": repo})),
    )
        .into_response()
}

/// Constant-time string comparison (via HMAC verification).
fn constant_time_eq(expected: &str, given: &str) -> bool {
    let Ok(mut a) = Hmac::<Sha256>::new_from_slice(b"wshm-token-compare") else {
        return false;
    };
    a.update(expected.as_bytes());
    let tag = a.finalize().into_bytes();
    let Ok(mut b) = Hmac::<Sha256>::new_from_slice(b"wshm-token-compare") else {
        return false;
    };
    b.update(given.as_bytes());
    b.verify_slice(&tag).is_ok()
}

/// Map a GitLab Issue / Merge Request / Note hook to the GitHub event the
/// processor handles (`issues.opened`, `pull_request.opened|synchronize`,
/// `issue_comment.created`). `None` for everything else.
fn translate_gitlab(gl_event: &str, p: &Value) -> Option<(String, String, Value)> {
    let full_name = p["project"]["path_with_namespace"].as_str()?.to_string();
    let attrs = &p["object_attributes"];
    let repo = json!({ "full_name": full_name });
    match gl_event {
        "Issue Hook" if attrs["action"] == "open" => {
            let iid = attrs["iid"].as_u64()?;
            Some((
                "issues".into(),
                "opened".into(),
                json!({"action": "opened", "issue": {"number": iid}, "repository": repo}),
            ))
        }
        "Merge Request Hook" => {
            let iid = attrs["iid"].as_u64()?;
            let action = match attrs["action"].as_str()? {
                "open" | "reopen" => "opened",
                // `oldrev` is only present when the update pushed commits.
                "update" if attrs.get("oldrev").is_some_and(|v| !v.is_null()) => "synchronize",
                _ => return None,
            };
            Some((
                "pull_request".into(),
                action.into(),
                json!({"action": action, "pull_request": {"number": iid}, "repository": repo}),
            ))
        }
        // Only new notes: an edited note must not re-run its command.
        "Note Hook" if attrs["action"].as_str().is_none_or(|a| a == "create") => {
            let (number, is_pr) = match attrs["noteable_type"].as_str()? {
                "Issue" => (p["issue"]["iid"].as_u64()?, false),
                "MergeRequest" => (p["merge_request"]["iid"].as_u64()?, true),
                _ => return None,
            };
            let user = p["user"]["username"].as_str().unwrap_or("").to_string();
            let mut issue = json!({ "number": number });
            if is_pr {
                issue["pull_request"] = json!({});
            }
            Some((
                "issue_comment".into(),
                "created".into(),
                json!({
                    "action": "created",
                    "issue": issue,
                    "comment": {"id": attrs["id"], "body": attrs["note"], "user": {"login": user}},
                    "sender": {"login": user},
                    "repository": repo,
                }),
            ))
        }
        _ => None,
    }
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

#[cfg(test)]
mod forge_webhook_tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    #[test]
    fn gitlab_token_is_checked() {
        let body = Bytes::from(
            json!({"project": {"path_with_namespace": "g/p"},
                   "object_attributes": {"action": "open", "iid": 3}})
            .to_string(),
        );
        // Only this test touches this variable.
        std::env::set_var("WSHM_GITLAB_WEBHOOK_TOKEN", "gl-token");
        let ok = headers(&[
            ("x-gitlab-event", "Issue Hook"),
            ("x-gitlab-token", "gl-token"),
        ]);
        let (ev, action, payload) = validate_webhook(Some("s3cret"), &ok, &body).unwrap();
        assert_eq!((ev.as_str(), action.as_str()), ("issues", "opened"));
        assert_eq!(extract_number(&ev, &payload), Some(3));
        assert_eq!(payload["repository"]["full_name"], "g/p");

        let bad = headers(&[("x-gitlab-event", "Issue Hook"), ("x-gitlab-token", "nope")]);
        assert!(validate_webhook(Some("s3cret"), &bad, &body).is_err());
        // The GitHub HMAC secret is not a valid GitLab token.
        let github_secret = headers(&[
            ("x-gitlab-event", "Issue Hook"),
            ("x-gitlab-token", "s3cret"),
        ]);
        assert!(validate_webhook(Some("s3cret"), &github_secret, &body).is_err());
        let missing = headers(&[("x-gitlab-event", "Issue Hook")]);
        assert!(validate_webhook(Some("s3cret"), &missing, &body).is_err());
    }

    #[test]
    fn delivery_format_must_match_repo_provider() {
        let cfg = |p: Option<&str>| crate::config::Config {
            git_provider: p.map(String::from),
            repo_owner: "rtk-ai".into(),
            repo_name: "rtk".into(),
            ..Default::default()
        };
        let gitlab = headers(&[("x-gitlab-event", "Note Hook")]);
        let github = headers(&[("x-github-event", "issue_comment")]);
        // A GitLab-format hook can never drive a GitHub (or Gitea) repo...
        assert!(!format_matches_provider(&gitlab, &cfg(None)));
        assert!(!format_matches_provider(&gitlab, &cfg(Some("gitea"))));
        // ...nor a GitHub-format one a GitLab repo; Azure takes neither.
        assert!(!format_matches_provider(&github, &cfg(Some("gitlab"))));
        assert!(!format_matches_provider(
            &github,
            &cfg(Some("azure-devops"))
        ));
        assert!(format_matches_provider(&gitlab, &cfg(Some("gitlab"))));
        assert!(format_matches_provider(&github, &cfg(None)));
        assert!(format_matches_provider(&github, &cfg(Some("forgejo"))));
    }

    #[test]
    fn gitlab_note_edit_is_ignored() {
        let p = json!({"project": {"path_with_namespace": "g/p"}, "user": {"username": "bob"},
                       "object_attributes": {"id": 9, "note": "/wshm fix", "noteable_type": "Issue", "action": "update"},
                       "issue": {"iid": 2}});
        assert!(translate_gitlab("Note Hook", &p).is_none());
        let mut created = p.clone();
        created["object_attributes"]["action"] = json!("create");
        assert!(translate_gitlab("Note Hook", &created).is_some());
    }

    #[test]
    fn gitlab_mr_and_note_translation() {
        let project = json!({"path_with_namespace": "g/p"});
        let (ev, action, _) = translate_gitlab(
            "Merge Request Hook",
            &json!({"project": project, "object_attributes": {"action": "update", "iid": 5, "oldrev": "abc"}}),
        )
        .unwrap();
        assert_eq!(
            (ev.as_str(), action.as_str()),
            ("pull_request", "synchronize")
        );
        // A title edit (no oldrev) is not a push.
        assert!(translate_gitlab(
            "Merge Request Hook",
            &json!({"project": project, "object_attributes": {"action": "update", "iid": 5}}),
        )
        .is_none());

        let (ev, action, payload) = translate_gitlab(
            "Note Hook",
            &json!({"project": project, "user": {"username": "bob"},
                    "object_attributes": {"id": 9, "note": "/wshm analyze", "noteable_type": "MergeRequest"},
                    "merge_request": {"iid": 5}}),
        )
        .unwrap();
        assert_eq!((ev.as_str(), action.as_str()), ("issue_comment", "created"));
        assert_eq!(payload["issue"]["number"], 5);
        assert!(payload["issue"].get("pull_request").is_some());
        assert_eq!(payload["comment"]["user"]["login"], "bob");
        assert_eq!(payload["comment"]["body"], "/wshm analyze");
    }

    #[test]
    fn gitea_synchronized_is_normalized() {
        let secret = "k";
        let body = Bytes::from(
            json!({"action": "synchronized", "pull_request": {"number": 2}}).to_string(),
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&body);
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        let h = headers(&[
            ("x-github-event", "pull_request"),
            ("x-hub-signature-256", &sig),
        ]);
        let (ev, action, _) = validate_webhook(Some(secret), &h, &body).unwrap();
        assert_eq!(
            (ev.as_str(), action.as_str()),
            ("pull_request", "synchronize")
        );
        assert!(should_process_event(&ev, &action));
    }
}
