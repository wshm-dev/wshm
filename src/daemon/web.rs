//! REST API endpoints and embedded Svelte SPA serving for the wshm web UI.
//!
//! This module provides:
//! - Basic auth middleware (checking against `[web]` config values)
//! - JSON API endpoints under `/api/v1/`
//! - Embedded SPA serving via `rust-embed` (files from `src/web-dist/`)
//!
//! NOTE: The `WebAssets` embed will fail to compile if `src/web-dist/` does not
//! contain any files. A placeholder `index.html` is provided for development.

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use hmac::{Hmac, Mac};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use std::sync::Arc;

use super::MultiDaemonState;

// ---------------------------------------------------------------------------
// Embedded SPA assets
// ---------------------------------------------------------------------------

#[derive(Embed)]
#[folder = "src/web-dist/"]
struct WebAssets;

// ---------------------------------------------------------------------------
// Shared state for web routes
// ---------------------------------------------------------------------------

/// Shared state for all web UI routes (OSS and Pro).
///
/// Exposed so that extension crates (e.g. `wshm-pro`) can build additional
/// `Router<Arc<WebState>>` routers and merge them into the main router via
/// [`web_routes_with_extensions`].
pub struct WebState {
    pub multi: Arc<MultiDaemonState>,
    /// Optional RBAC store. `Some` enables multi-user accounts with roles
    /// (admin/member/viewer); `None` keeps the legacy single-credential
    /// `[web].username/password` Basic Auth flow.
    pub users: Option<Arc<crate::auth::UserStore>>,
    /// Optional in-memory log buffer fed by the tracing layer. When `Some`,
    /// `GET /api/v1/logs` returns the daemon's recent log lines.
    pub logs: Option<Arc<crate::daemon::log_buffer::LogBuffer>>,
}

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RepoFilter {
    repo: Option<String>,
}

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

/// Returns true if the request path can be served without auth (login page,
/// health probe, login API, static SPA assets).  Keeps the unauthenticated
/// browsing surface tight: anything else requires a valid session.
fn is_public_path(path: &str) -> bool {
    if path == "/health"
        || path == "/login"
        || path == "/api/v1/auth/login"
        || path == "/api/v1/auth/logout"
    {
        return true;
    }
    if path.starts_with("/_app/") {
        return true;
    }
    matches!(
        path,
        "/favicon.png" | "/favicon.ico" | "/wizard-icon.png" | "/robots.txt"
    )
}

/// Reads a cookie value by name out of the `Cookie:` header, if present.
fn read_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{name}=");
    raw.split(';')
        .map(|c| c.trim())
        .find_map(|c| c.strip_prefix(prefix.as_str()))
}

/// Mints a signed session cookie value: `<expires_unix>.<base64url_hmac>`.
/// The HMAC key is derived from the configured web password, so rotating the
/// password automatically invalidates every outstanding session.
fn mint_session_cookie(password: &str, ttl_secs: i64) -> (String, i64) {
    let expires_at = chrono::Utc::now().timestamp() + ttl_secs;
    let payload = expires_at.to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(password.as_bytes())
        .expect("HMAC accepts any key size");
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig);
    (format!("{payload}.{sig_b64}"), expires_at)
}

/// Verifies a session cookie value previously minted by `mint_session_cookie`.
/// Constant-time signature comparison; rejects expired tokens.
fn verify_session_cookie(password: &str, value: &str) -> bool {
    let (expires_str, sig_b64) = match value.split_once('.') {
        Some(p) => p,
        None => return false,
    };
    let expires_at: i64 = match expires_str.parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    if expires_at <= chrono::Utc::now().timestamp() {
        return false;
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(password.as_bytes())
        .expect("HMAC accepts any key size");
    mac.update(expires_str.as_bytes());
    let expected_sig = mac.finalize().into_bytes();
    let expected_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(expected_sig);
    if expected_b64.len() != sig_b64.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected_b64.bytes().zip(sig_b64.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// HMAC signing key for the user-id session cookie. Deployments using the
/// RBAC mode are expected to set `WSHM_JWT_SECRET`; the fallback only exists
/// so the daemon boots in dev — sessions then become trivially forgeable,
/// which is fine because `[web].password` is the real ACL in that mode.
fn user_cookie_secret() -> String {
    std::env::var("WSHM_JWT_SECRET").unwrap_or_else(|_| "wshm-cookie-fallback".to_string())
}

/// Mints a session cookie carrying a user id: `<user_id>.<expires>.<sig>`.
pub fn mint_user_cookie(user_id: i64, ttl_secs: i64) -> String {
    let expires_at = chrono::Utc::now().timestamp() + ttl_secs;
    let payload = format!("{user_id}.{expires_at}");
    let mut mac = Hmac::<Sha256>::new_from_slice(user_cookie_secret().as_bytes())
        .expect("HMAC accepts any key size");
    mac.update(payload.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sig);
    format!("{payload}.{sig_b64}")
}

/// Verifies a user-id cookie minted by [`mint_user_cookie`]. Returns the
/// user id if signature + expiry are valid.
pub fn verify_user_cookie(value: &str) -> Option<i64> {
    let parts: Vec<&str> = value.splitn(3, '.').collect();
    if parts.len() != 3 {
        return None;
    }
    let user_id: i64 = parts[0].parse().ok()?;
    let expires_at: i64 = parts[1].parse().ok()?;
    if expires_at <= chrono::Utc::now().timestamp() {
        return None;
    }
    let payload = format!("{}.{}", parts[0], parts[1]);
    let mut mac = Hmac::<Sha256>::new_from_slice(user_cookie_secret().as_bytes())
        .expect("HMAC accepts any key size");
    mac.update(payload.as_bytes());
    let expected_sig = mac.finalize().into_bytes();
    let expected_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(expected_sig);
    if expected_b64.len() != parts[2].len() {
        return None;
    }
    let mut diff = 0u8;
    for (a, b) in expected_b64.bytes().zip(parts[2].bytes()) {
        diff |= a ^ b;
    }
    (diff == 0).then_some(user_id)
}

/// Identify the requesting user when RBAC is enabled. Tries the user-id
/// cookie first, falls back to `X-Auth-Request-Email` / `X-Forwarded-Email`
/// (oauth2-proxy SSO), upserting the SSO row on first sight. Returns `None`
/// when no users store is configured or the request has no usable identity.
async fn current_user(
    state: &Arc<WebState>,
    headers: &HeaderMap,
) -> Option<crate::auth::User> {
    let store = state.users.as_ref()?;

    if let Some(cookie_val) = read_cookie(headers, "wshm_session") {
        if let Some(uid) = verify_user_cookie(cookie_val) {
            if let Ok(Some(u)) = store.find_by_id(uid).await {
                return Some(u);
            }
        }
    }

    let trusts_proxy = std::env::var("WSHM_TRUST_PROXY_AUTH")
        .ok()
        .filter(|v| v == "1" || v == "true")
        .is_some();
    if trusts_proxy {
        let email = headers
            .get("x-auth-request-email")
            .or_else(|| headers.get("x-forwarded-email"))
            .and_then(|v| v.to_str().ok());
        if let Some(email) = email {
            let username = headers
                .get("x-forwarded-user")
                .or_else(|| headers.get("x-forwarded-preferred-username"))
                .and_then(|v| v.to_str().ok());
            if let Ok(u) = store.upsert_sso(email, username, "google").await {
                return Some(u);
            }
        }
    }

    None
}

/// Auth middleware that accepts, in order:
/// - public paths (`/health`, `/login`, `/api/v1/auth/login`, static assets);
/// - in RBAC mode (state.users is Some): a user-id cookie or oauth2-proxy
///   headers resolved via [`current_user`];
/// - a valid `wshm_session` cookie (set by POST /api/v1/auth/login);
/// - oauth2-proxy forwarded headers when `WSHM_TRUST_PROXY_AUTH=1`;
/// - HTTP Basic Auth (kept for CLI/curl callers).
///
/// Browser HTML requests get a 302 redirect to `/login`; everything else
/// (API/JSON) gets a 401 with a JSON error body. Apps in the SPA detect the
/// 302 and render the login form.
async fn auth_middleware(
    State(state): State<Arc<WebState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    if is_public_path(&path) {
        return next.run(req).await;
    }

    // RBAC mode: a UserStore is configured. Resolve identity through the
    // user-id cookie or oauth2-proxy headers, attach the User to request
    // extensions so handlers can read it.
    if state.users.is_some() {
        if let Some(user) = current_user(&state, req.headers()).await {
            req.extensions_mut()
                .insert(Some(user) as Option<crate::auth::User>);
            return next.run(req).await;
        }
        // Fall through to the legacy paths below: a CLI client may still be
        // hitting the API with Basic Auth even though local accounts exist.
    }
    req.extensions_mut()
        .insert(None as Option<crate::auth::User>);

    let repos = state.multi.repos.read().await;
    let web_cfg = match repos.values().next() {
        Some(ds) => ds.config.web.clone(),
        None => {
            drop(repos);
            return next.run(req).await;
        }
    };
    drop(repos);

    // No password configured → auth disabled (single-user dev mode).
    let required_password = match &web_cfg.password {
        Some(p) => p.clone(),
        None => return next.run(req).await,
    };
    let expected_username = web_cfg.username.clone();

    // 1) Trust oauth2-proxy headers when explicitly enabled. Looking for
    //    X-Forwarded-User / X-Forwarded-Email / X-Auth-Request-Email which
    //    oauth2-proxy sets after a successful SSO.
    if std::env::var("WSHM_TRUST_PROXY_AUTH")
        .ok()
        .filter(|v| v == "1" || v == "true")
        .is_some()
    {
        let has_proxy_header = req.headers().keys().any(|k| {
            let n = k.as_str().to_ascii_lowercase();
            n == "x-forwarded-user" || n == "x-forwarded-email" || n == "x-auth-request-email"
        });
        if has_proxy_header {
            return next.run(req).await;
        }
    }

    // 2) Signed session cookie set by /api/v1/auth/login. Skipped in RBAC
    //    mode — there the user-id cookie is the canonical session and was
    //    already tried above via current_user().
    if state.users.is_none() {
        if let Some(cookie_val) = read_cookie(req.headers(), "wshm_session") {
            if verify_session_cookie(&required_password, cookie_val) {
                return next.run(req).await;
            }
        }
    }

    // 3) Basic Auth fallback (CLI / curl).
    let basic_ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|decoded| {
            if let Some((user, pass)) = decoded.split_once(':') {
                user == expected_username && pass == required_password
            } else {
                false
            }
        })
        .unwrap_or(false);
    if basic_ok {
        return next.run(req).await;
    }

    // Decide between 302 (browser → /login) and 401 (API/curl).
    // Heuristic: API paths and Accept: application/json get JSON; everything
    // else gets a redirect so the SPA can render the login page.
    let accept = req
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let wants_json = path.starts_with("/api/") || accept.contains("application/json");

    if wants_json {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"wshm\"")],
            Json(json!({"error": "unauthorized"})),
        )
            .into_response()
    } else {
        (
            StatusCode::FOUND,
            [(header::LOCATION, HeaderValue::from_static("/login"))],
        )
            .into_response()
    }
}

/// POST /api/v1/auth/login -- validate credentials and set a session cookie.
///
/// In RBAC mode (state.users is Some) the credentials are looked up in the
/// UserStore (email or username) and verified with argon2; the cookie
/// encodes the user id and is signed with `WSHM_JWT_SECRET`.
///
/// In legacy mode the credentials are checked against the configured
/// `[web].username/password` and the cookie is HMAC-keyed with the password.
///
/// In both modes the cookie is HttpOnly + Secure + SameSite=Lax + Path=/
/// with a 7-day TTL.
async fn api_auth_login(
    State(state): State<Arc<WebState>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let username = body
        .get("username")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let password = body
        .get("password")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(store) = state.users.as_ref() {
        let lookup = match store.find_by_login(&username).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("user lookup failed: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "internal error"})),
                )
                    .into_response();
            }
        };
        let (user, hash) = match lookup {
            Some((u, Some(h))) => (u, h),
            _ => {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "invalid credentials"})),
                )
                    .into_response();
            }
        };
        if !crate::auth::verify_password(&password, &hash) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "invalid credentials"})),
            )
                .into_response();
        }
        if let Err(e) = store.touch_login(user.id).await {
            tracing::warn!("touch_login: {e}");
        }
        let cookie_val = mint_user_cookie(user.id, 7 * 24 * 3600);
        let cookie_header = format!(
            "wshm_session={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
            cookie_val,
            7 * 24 * 3600
        );
        return (
            StatusCode::OK,
            [(header::SET_COOKIE, cookie_header)],
            Json(json!({"status": "ok", "role": user.role.as_str()})),
        )
            .into_response();
    }

    // Legacy single-credential path.
    let repos = state.multi.repos.read().await;
    let web_cfg = match repos.values().next() {
        Some(ds) => ds.config.web.clone(),
        None => {
            drop(repos);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "no repo configured"})),
            )
                .into_response();
        }
    };
    drop(repos);

    let required_password = match web_cfg.password.as_ref() {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "web auth disabled (no password configured)"})),
            )
                .into_response();
        }
    };

    if username != web_cfg.username || password != required_password {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid credentials"})),
        )
            .into_response();
    }

    let (cookie_val, _expires_at) = mint_session_cookie(&required_password, 7 * 24 * 3600);
    let cookie_header = format!(
        "wshm_session={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
        cookie_val,
        7 * 24 * 3600
    );

    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie_header)],
        Json(json!({"status": "ok"})),
    )
        .into_response()
}

/// GET /api/v1/auth/me -- return the identity of the current user.
///
/// In RBAC mode resolves the User attached by [`auth_middleware`] (cookie or
/// SSO header) and returns id + role. Otherwise reads oauth2-proxy headers
/// for SSO identity, or falls back to the `[web].username`.
async fn api_auth_me(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    user: axum::Extension<Option<crate::auth::User>>,
) -> Response {
    if state.users.is_some() {
        if let Some(u) = user.0 {
            return Json(json!({
                "id": u.id,
                "email": u.email,
                "username": u.username,
                "role": u.role.as_str(),
                "auth_method": if u.sso_provider.is_some() { "sso" } else { "local" },
            }))
            .into_response();
        }
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        )
            .into_response();
    }

    let email = headers
        .get("x-auth-request-email")
        .or_else(|| headers.get("x-forwarded-email"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let user = headers
        .get("x-forwarded-user")
        .or_else(|| headers.get("x-forwarded-preferred-username"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if email.is_some() || user.is_some() {
        return Json(json!({
            "email": email,
            "username": user,
            "auth_method": "sso",
        }))
        .into_response();
    }

    let repos = state.multi.repos.read().await;
    let username = repos
        .values()
        .next()
        .map(|ds| ds.config.web.username.clone())
        .unwrap_or_else(|| "user".to_string());
    drop(repos);

    Json(json!({
        "email": null,
        "username": username,
        "auth_method": "local",
    }))
    .into_response()
}

/// Helper for admin-gated handlers. Returns Err response when the request is
/// not authenticated or the user is not an admin.
fn require_admin(
    user: &axum::Extension<Option<crate::auth::User>>,
) -> Result<crate::auth::User, Response> {
    match &user.0 {
        Some(u) if u.role == crate::auth::Role::Admin => Ok(u.clone()),
        Some(_) => Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "admin role required"})),
        )
            .into_response()),
        None => Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        )
            .into_response()),
    }
}

/// GET /api/v1/users -- list all users (admin only).
async fn api_users_list(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
) -> Response {
    if let Err(e) = require_admin(&user) {
        return e;
    }
    let store = match state.users.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "RBAC not configured"})),
            )
                .into_response();
        }
    };
    match store.list().await {
        Ok(users) => Json(json!({ "users": users })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// POST /api/v1/users -- create a new local-credential user (admin only).
/// Body: `{email, username?, password, role}`.
async fn api_users_create(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(e) = require_admin(&user) {
        return e;
    }
    let store = match state.users.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "RBAC not configured"})),
            )
                .into_response();
        }
    };
    let email = body.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let username = body.get("username").and_then(|v| v.as_str());
    let password = body.get("password").and_then(|v| v.as_str()).unwrap_or("");
    let role_str = body
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or("member");
    if email.trim().is_empty() || password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "email and password are required"})),
        )
            .into_response();
    }
    let role = match crate::auth::Role::from_str(role_str) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("{e}")})),
            )
                .into_response();
        }
    };
    match store.create_local(email, username, password, role).await {
        Ok(id) => (StatusCode::CREATED, Json(json!({ "id": id }))).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// PATCH /api/v1/users/{id} -- update user role and/or password (admin only).
/// Body: `{role?, password?}`.
async fn api_users_update(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(e) = require_admin(&user) {
        return e;
    }
    let store = match state.users.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "RBAC not configured"})),
            )
                .into_response();
        }
    };
    if let Some(role_str) = body.get("role").and_then(|v| v.as_str()) {
        let role = match crate::auth::Role::from_str(role_str) {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("{e}")})),
                )
                    .into_response();
            }
        };
        if let Err(e) = store.update_role(id, role).await {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("{e}")})),
            )
                .into_response();
        }
    }
    if let Some(pw) = body.get("password").and_then(|v| v.as_str()) {
        if !pw.is_empty() {
            if let Err(e) = store.update_password(id, pw).await {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("{e}")})),
                )
                    .into_response();
            }
        }
    }
    Json(json!({"status": "ok"})).into_response()
}

/// DELETE /api/v1/users/{id} -- remove a user (admin only).
async fn api_users_delete(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    let admin = match require_admin(&user) {
        Ok(u) => u,
        Err(e) => return e,
    };
    if admin.id == id {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "cannot delete yourself"})),
        )
            .into_response();
    }
    let store = match state.users.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "RBAC not configured"})),
            )
                .into_response();
        }
    };
    match store.delete(id).await {
        Ok(_) => Json(json!({"status": "ok"})).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// POST /api/v1/auth/logout -- clear the `wshm_session` cookie.
async fn api_auth_logout() -> Response {
    let cookie_header =
        "wshm_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0".to_string();
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie_header)],
        Json(json!({"status": "ok"})),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct RepoStatus {
    slug: String,
    open_issues: usize,
    untriaged: usize,
    open_prs: usize,
    unanalyzed: usize,
    conflicts: usize,
    last_sync: Option<String>,
    apply: bool,
}

#[derive(Serialize)]
struct StatusResponse {
    open_issues: usize,
    untriaged: usize,
    open_prs: usize,
    unanalyzed: usize,
    conflicts: usize,
    last_sync: Option<String>,
    repos: Vec<RepoStatus>,
}

#[derive(Serialize)]
struct ActivityEntry {
    #[serde(rename = "type")]
    entry_type: String,
    repo: String,
    number: u64,
    summary: Option<String>,
    category: Option<String>,
    risk_level: Option<String>,
    at: String,
}

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/status -- aggregate status across all repos (or per-repo).
async fn api_status(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut resp = StatusResponse {
        open_issues: 0,
        untriaged: 0,
        open_prs: 0,
        unanalyzed: 0,
        conflicts: 0,
        last_sync: None,
        repos: Vec::new(),
    };

    let __repos_guard = state.multi.repos.read().await;
    for (slug, ds) in __repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }

        let open_issues = ds.db.get_open_issues().unwrap_or_default();
        let untriaged = ds.db.get_untriaged_issues().unwrap_or_default();
        let open_prs = ds.db.get_open_pulls().unwrap_or_default();
        let unanalyzed = ds.db.get_unanalyzed_pulls().unwrap_or_default();

        let conflicts = open_prs
            .iter()
            .filter(|pr| pr.mergeable == Some(false))
            .count();

        let last_sync = ds
            .db
            .get_sync_entry("issues")
            .ok()
            .flatten()
            .map(|e| e.last_synced_at);

        let repo_status = RepoStatus {
            slug: slug.clone(),
            open_issues: open_issues.len(),
            untriaged: untriaged.len(),
            open_prs: open_prs.len(),
            unanalyzed: unanalyzed.len(),
            conflicts,
            last_sync: last_sync.clone(),
            apply: ds.apply,
        };

        resp.open_issues += repo_status.open_issues;
        resp.untriaged += repo_status.untriaged;
        resp.open_prs += repo_status.open_prs;
        resp.unanalyzed += repo_status.unanalyzed;
        resp.conflicts += repo_status.conflicts;

        // Use the most recent sync time across repos
        if let Some(ref ls) = last_sync {
            if resp.last_sync.as_ref().is_none_or(|cur| ls > cur) {
                resp.last_sync = Some(ls.clone());
            }
        }

        resp.repos.push(repo_status);
    }

    Json(resp)
}

/// GET /api/v1/issues -- open issues from DB.
async fn api_issues(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut all_issues = Vec::new();

    let __repos_guard = state.multi.repos.read().await;
    for (slug, ds) in __repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }
        if let Ok(issues) = ds.db.get_open_issues() {
            // Build a map: issue_number -> list of linked PRs (from open PRs bodies)
            let open_prs = ds.db.get_open_pulls().unwrap_or_default();
            let mut issue_prs: std::collections::HashMap<u64, Vec<serde_json::Value>> =
                std::collections::HashMap::new();
            for pr in &open_prs {
                let body = pr.body.as_deref().unwrap_or("");
                let linked = crate::pipelines::extract_linked_issue_numbers(body);
                for issue_num in linked {
                    issue_prs.entry(issue_num).or_default().push(json!({
                        "number": pr.number,
                        "title": pr.title,
                        "state": pr.state,
                        "draft": pr.title.to_lowercase().contains("[draft]") || pr.labels.iter().any(|l| l.to_lowercase().contains("draft")),
                        "ci_status": pr.ci_status,
                        "mergeable": pr.mergeable,
                    }));
                }
            }

            for issue in issues {
                let triage = ds.db.get_triage_result(issue.number).ok().flatten();
                let linked = issue_prs.get(&issue.number);
                let pr_status = match linked {
                    None => "no_pr",
                    Some(prs) => {
                        let has_ready = prs.iter().any(|p| {
                            let ci_ok = p["ci_status"]
                                .as_str()
                                .map(|s| s == "success")
                                .unwrap_or(false);
                            let mergeable = p["mergeable"].as_bool().unwrap_or(true);
                            let not_draft = !p["draft"].as_bool().unwrap_or(false);
                            ci_ok && mergeable && not_draft
                        });
                        if has_ready {
                            "pr_ready"
                        } else {
                            "has_pr"
                        }
                    }
                };
                all_issues.push(json!({
                    "repo": slug,
                    "number": issue.number,
                    "title": issue.title,
                    "body": issue.body,
                    "state": issue.state,
                    "labels": issue.labels,
                    "author": issue.author,
                    "created_at": issue.created_at,
                    "updated_at": issue.updated_at,
                    "reactions_plus1": issue.reactions_plus1,
                    "reactions_total": issue.reactions_total,
                    "priority": triage.as_ref().and_then(|t| t.priority.as_deref()),
                    "category": triage.as_ref().map(|t| t.category.as_str()),
                    "pr_status": pr_status,
                    "linked_prs": linked,
                }));
            }
        }
    }

    Json(all_issues)
}

/// GET /api/v1/pulls -- open PRs from DB.
async fn api_pulls(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut all_prs = Vec::new();

    let __repos_guard = state.multi.repos.read().await;
    for (slug, ds) in __repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }
        if let Ok(prs) = ds.db.get_open_pulls() {
            for pr in prs {
                let analysis = ds.db.get_pr_analysis(pr.number).ok().flatten();
                all_prs.push(json!({
                    "repo": slug,
                    "number": pr.number,
                    "title": pr.title,
                    "body": pr.body,
                    "state": pr.state,
                    "labels": pr.labels,
                    "author": pr.author,
                    "head_ref": pr.head_ref,
                    "base_ref": pr.base_ref,
                    "mergeable": pr.mergeable,
                    "ci_status": pr.ci_status,
                    "created_at": pr.created_at,
                    "updated_at": pr.updated_at,
                    "risk_level": analysis.as_ref().map(|a| a.risk_level.as_str()),
                    "pr_type": analysis.as_ref().map(|a| a.pr_type.as_str()),
                    "summary": analysis.as_ref().map(|a| a.summary.as_str()),
                }));
            }
        }
    }

    Json(all_prs)
}

/// GET /api/v1/triage -- recent triage results.
async fn api_triage(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut all_results = Vec::new();

    let __repos_guard = state.multi.repos.read().await;
    for (slug, ds) in __repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }
        if let Ok(results) = ds.db.recent_activity(50) {
            for r in results {
                all_results.push(json!({
                    "repo": slug,
                    "issue_number": r.issue_number,
                    "category": r.category,
                    "confidence": r.confidence,
                    "priority": r.priority,
                    "summary": r.summary,
                    "is_simple_fix": r.is_simple_fix,
                    "acted_at": r.acted_at,
                }));
            }
        }
    }

    Json(all_results)
}

/// GET /api/v1/queue -- merge queue: open PRs with basic scoring data.
async fn api_queue(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut queue = Vec::new();

    let __repos_guard = state.multi.repos.read().await;
    for (slug, ds) in __repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }
        if let Ok(prs) = ds.db.get_open_pulls() {
            for pr in prs {
                // Basic scoring (mirrors pipelines::merge_queue logic)
                let mut score: i64 = 0;

                // CI passing
                if pr.ci_status.as_deref() == Some("success") {
                    score += 10;
                }

                // Conflicts
                if pr.mergeable == Some(false) {
                    score -= 10;
                }

                // Staleness bonus: +1 per day since creation, max 10
                if let Ok(created) = chrono::DateTime::parse_from_rfc3339(&pr.created_at) {
                    let age_days = (chrono::Utc::now() - created.with_timezone(&chrono::Utc))
                        .num_days()
                        .min(10);
                    score += age_days;
                }

                // Analysis data (if available)
                let analysis = ds.db.get_pr_analysis(pr.number).ok().flatten();
                if let Some(ref a) = analysis {
                    match a.risk_level.as_str() {
                        "low" => score += 5,
                        "high" => score -= 5,
                        _ => {}
                    }
                }

                queue.push(json!({
                    "repo": slug,
                    "number": pr.number,
                    "title": pr.title,
                    "author": pr.author,
                    "mergeable": pr.mergeable,
                    "ci_status": pr.ci_status,
                    "score": score,
                    "risk_level": analysis.as_ref().map(|a| &a.risk_level),
                    "pr_type": analysis.as_ref().map(|a| &a.pr_type),
                    "created_at": pr.created_at,
                }));
            }
        }
    }

    // Sort descending by score
    queue.sort_by(|a, b| {
        let sa = a.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        let sb = b.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        sb.cmp(&sa)
    });

    Json(queue)
}

/// GET /api/v1/activity -- combined recent triage + PR analysis activity.
async fn api_activity(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut entries: Vec<ActivityEntry> = Vec::new();

    let __repos_guard = state.multi.repos.read().await;
    for (slug, ds) in __repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }

        // Triage activity
        if let Ok(results) = ds.db.recent_activity(25) {
            for r in results {
                entries.push(ActivityEntry {
                    entry_type: "triage".to_string(),
                    repo: slug.clone(),
                    number: r.issue_number,
                    summary: r.summary.clone(),
                    category: Some(r.category),
                    risk_level: None,
                    at: r.acted_at,
                });
            }
        }

        // PR analysis activity -- iterate open PRs and check for analyses
        if let Ok(prs) = ds.db.get_open_pulls() {
            for pr in prs {
                if let Ok(Some(a)) = ds.db.get_pr_analysis(pr.number) {
                    entries.push(ActivityEntry {
                        entry_type: "pr_analysis".to_string(),
                        repo: slug.clone(),
                        number: pr.number,
                        summary: Some(a.summary),
                        category: Some(a.pr_type),
                        risk_level: Some(a.risk_level),
                        at: a.analyzed_at,
                    });
                }
            }
        }
    }

    // Sort by timestamp descending
    entries.sort_by(|a, b| b.at.cmp(&a.at));
    entries.truncate(50);

    Json(entries)
}

// ---------------------------------------------------------------------------
// SPA serving
// ---------------------------------------------------------------------------

/// Serve a static file from the embedded assets.
async fn serve_asset(path: &str) -> Response {
    // Try exact path first
    if let Some(file) = WebAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime.as_ref().to_string())],
            file.data.to_vec(),
        )
            .into_response()
    } else {
        // SPA fallback: return index.html for any non-API, non-asset route
        serve_index().await
    }
}

/// Return the embedded index.html.
async fn serve_index() -> Response {
    match WebAssets::get("index.html") {
        Some(file) => Html(String::from_utf8_lossy(&file.data).to_string()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "index.html not found in embedded assets",
        )
            .into_response(),
    }
}

/// Handler for GET / -- serves the SPA entry point.
async fn handle_spa_root() -> Response {
    serve_index().await
}

/// Fallback handler for all non-matched routes -- serves SPA or static asset.
async fn handle_spa_fallback(req: Request<Body>) -> Response {
    let path = req.uri().path().trim_start_matches('/');

    if path.is_empty() {
        return serve_index().await;
    }

    serve_asset(path).await
}

/// GET /api/v1/changelog -- changelog from closed/merged PRs.
async fn api_changelog(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut sections: std::collections::HashMap<String, Vec<serde_json::Value>> =
        std::collections::HashMap::new();

    let __repos_guard = state.multi.repos.read().await;
    for (slug, ds) in __repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }

        let closed_prs = ds.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT number, title, labels, author, updated_at
                 FROM pull_requests WHERE state = 'closed'
                 ORDER BY updated_at DESC LIMIT 100",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    let labels_str: String = row.get(2)?;
                    Ok(json!({
                        "number": row.get::<_, u64>(0)?,
                        "title": row.get::<_, String>(1)?,
                        "labels": serde_json::from_str::<Vec<String>>(&labels_str).unwrap_or_default(),
                        "author": row.get::<_, Option<String>>(3)?,
                        "merged_at": row.get::<_, String>(4)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        });

        if let Ok(prs) = closed_prs {
            for pr in prs {
                let title = pr["title"].as_str().unwrap_or("");
                let labels: Vec<String> = pr["labels"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let section = if title.starts_with("feat") {
                    "Features"
                } else if title.starts_with("fix") {
                    "Bug Fixes"
                } else if title.starts_with("docs") {
                    "Documentation"
                } else if title.starts_with("refactor") {
                    "Refactoring"
                } else if title.starts_with("chore")
                    || title.starts_with("ci")
                    || title.starts_with("build")
                {
                    "Maintenance"
                } else if labels
                    .iter()
                    .any(|l| l.contains("feature") || l.contains("enhancement"))
                {
                    "Features"
                } else if labels
                    .iter()
                    .any(|l| l.contains("bug") || l.contains("fix"))
                {
                    "Bug Fixes"
                } else if labels.iter().any(|l| l.contains("docs")) {
                    "Documentation"
                } else {
                    "Other"
                };

                let mut entry = pr.clone();
                entry["repo"] = json!(slug);
                sections.entry(section.to_string()).or_default().push(entry);
            }
        }
    }

    let order = [
        "Features",
        "Bug Fixes",
        "Refactoring",
        "Documentation",
        "Maintenance",
        "Other",
    ];
    let result: Vec<serde_json::Value> = order
        .iter()
        .filter_map(|name| {
            sections.get(*name).map(|prs| {
                json!({
                    "name": name,
                    "pull_requests": prs,
                })
            })
        })
        .collect();

    Json(json!({ "sections": result }))
}

/// GET /api/v1/revert/preview -- preview what revert would do.
async fn api_revert_preview(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut preview = Vec::new();

    let __repos_guard = state.multi.repos.read().await;
    for (slug, ds) in __repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }

        let mut comments_count = 0usize;
        let mut labels_count = 0usize;

        if let Ok(issues) = ds.db.get_open_issues() {
            for issue in &issues {
                if let Ok(labels) = ds.db.get_wshm_applied_labels(issue.number) {
                    if !labels.is_empty() {
                        labels_count += labels.len();
                    }
                }
            }
        }

        if let Ok(results) = ds.db.recent_activity(1000) {
            comments_count = results.len();
        }

        let pr_analyses_count = ds
            .db
            .get_open_pulls()
            .unwrap_or_default()
            .iter()
            .filter(|pr| ds.db.get_pr_analysis(pr.number).ok().flatten().is_some())
            .count();

        preview.push(json!({
            "repo": slug,
            "triage_results": comments_count,
            "pr_analyses": pr_analyses_count,
            "labels_to_remove": labels_count,
        }));
    }

    Json(json!({ "repos": preview }))
}

/// GET /api/v1/backups -- list backup files.
async fn api_list_backups() -> impl IntoResponse {
    let wshm_dir = std::path::PathBuf::from(".wshm");
    let mut backups = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&wshm_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("backup-") && name.ends_with(".tar.gz") {
                if let Ok(meta) = entry.metadata() {
                    let size = meta.len();
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| {
                            chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_default()
                        })
                        .unwrap_or_default();
                    backups.push(json!({
                        "name": name,
                        "path": entry.path().to_string_lossy(),
                        "size": size,
                        "created_at": modified,
                    }));
                }
            }
        }
    }

    backups.sort_by(|a, b| {
        let na = a["name"].as_str().unwrap_or("");
        let nb = b["name"].as_str().unwrap_or("");
        nb.cmp(na)
    });

    Json(json!({ "backups": backups }))
}

/// POST /api/v1/backup -- create a backup.
async fn api_create_backup() -> impl IntoResponse {
    let args = crate::cli::BackupArgs {
        output: None,
        include_logs: false,
    };
    match crate::pipelines::backup::backup(&args) {
        Ok(()) => Json(json!({"status": "ok", "message": "Backup created"})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "message": format!("{e}")})),
        )
            .into_response(),
    }
}

/// POST /api/v1/restore -- restore from backup.
async fn api_restore_backup(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    let path = match body.get("path").and_then(|v| v.as_str()) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "error",
                    "message": "path is required",
                })),
            )
                .into_response()
        }
    };

    let args = crate::cli::RestoreArgs {
        file: path,
        force: true,
    };
    match crate::pipelines::backup::restore(&args) {
        Ok(()) => Json(json!({"status": "ok", "message": "Restore complete"})).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "message": format!("{e}")})),
        )
            .into_response(),
    }
}

/// GET /api/v1/license -- license status and feature gates.
async fn api_license() -> impl IntoResponse {
    let is_pro = crate::pro_hooks::is_pro();

    let pro_features = [
        (
            "review",
            "Inline code review",
            is_pro && crate::pro_hooks::has_feature("review"),
        ),
        (
            "auto-fix",
            "Auto-generate fix PRs",
            is_pro && crate::pro_hooks::has_feature("auto-fix"),
        ),
        (
            "improve",
            "Propose improvements",
            is_pro && crate::pro_hooks::has_feature("improve"),
        ),
        (
            "conflicts",
            "Conflict resolution",
            is_pro && crate::pro_hooks::has_feature("conflicts"),
        ),
        (
            "reports",
            "HTML/PDF reports",
            is_pro && crate::pro_hooks::has_feature("reports"),
        ),
        (
            "daemon-webhook",
            "Daemon webhook mode",
            is_pro && crate::pro_hooks::has_feature("daemon"),
        ),
    ];

    let features: Vec<serde_json::Value> = pro_features
        .iter()
        .map(|(id, label, enabled)| json!({ "id": id, "label": label, "enabled": enabled }))
        .collect();

    Json(json!({
        "is_pro": is_pro,
        "plan": if is_pro { "pro" } else { "free" },
        "features": features,
        "oss_features": [
            "triage", "pr_analysis", "merge_queue", "notify",
            "web_ui", "daemon_polling", "sqlite", "postgresql"
        ],
    }))
}

/// POST /api/v1/license -- activate a license key from the web UI.
async fn api_license_activate(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    let key = match body.get("license_key").and_then(|v| v.as_str()) {
        Some(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "license_key is required"})),
            )
                .into_response()
        }
    };

    // Try to activate via the license module
    match activate_license(&key) {
        Ok(plan) => Json(json!({
            "status": "ok",
            "plan": plan,
            "message": "License activated successfully",
        }))
        .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "message": format!("{e}"),
            })),
        )
            .into_response(),
    }
}

/// Activate a license key: save to credentials, call API, cache JWT.
fn activate_license(key: &str) -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let machine_id = {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_default();
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(format!("{hostname}:{username}"));
        format!("{:x}", hasher.finalize())
    };

    let resp = ureq::post("https://api.wshm.dev/api/v1/license/activate")
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send_string(
            &serde_json::json!({
                "license_key": key,
                "machine_id": machine_id,
                "app_version": env!("CARGO_PKG_VERSION"),
            })
            .to_string(),
        )
        .map_err(|e| format!("Cannot reach license server: {e}"))?;

    let body: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("Invalid response: {e}"))?;

    if let Some(token) = body["token"].as_str() {
        // Cache JWT
        let path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".wshm")
            .join("license.jwt");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        let plan = body["license"]["type"]
            .as_str()
            .unwrap_or("pro")
            .to_string();
        Ok(plan)
    } else {
        Err(body["error"]
            .as_str()
            .unwrap_or("Activation failed")
            .to_string())
    }
}

/// GET /api/v1/repos -- list configured repos.
async fn api_list_repos(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    let repos = state.multi.repos.read().await;
    let list: Vec<serde_json::Value> = repos
        .iter()
        .map(|(slug, ds)| {
            json!({
                "slug": slug,
                "apply": ds.apply,
                "wshm_dir": ds.config.wshm_dir.display().to_string(),
            })
        })
        .collect();
    let dynamic = state.multi.runtime.is_some();
    Json(json!({ "repos": list, "dynamic_add_supported": dynamic }))
}

/// POST /api/v1/repos -- add a repo at runtime (multi-repo mode only).
/// Body: {"slug": "owner/repo", "path": "/optional/abs/path"}
async fn api_add_repo(
    State(state): State<Arc<WebState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let slug = match body.get("slug").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "slug is required (format: owner/repo)"})),
            )
                .into_response();
        }
    };

    // Path: explicit if provided, else add_repo derives it from the runtime
    // config dir so per-repo state lives on the same volume as global.toml.
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from);

    match state.multi.add_repo(&slug, path).await {
        Ok(state) => (
            StatusCode::CREATED,
            Json(json!({
                "status": "ok",
                "slug": slug,
                "path": state.config.wshm_dir
                    .parent()
                    .unwrap_or(&state.config.wshm_dir)
                    .display()
                    .to_string(),
                "message": "Repo added — scheduler/poller spawned",
            })),
        )
            .into_response(),
        Err(e) => {
            let msg = format!("{e}");
            let code = if msg.contains("already") {
                StatusCode::CONFLICT
            } else if msg.contains("not available") {
                StatusCode::METHOD_NOT_ALLOWED
            } else {
                StatusCode::BAD_REQUEST
            };
            (code, Json(json!({"status": "error", "message": msg}))).into_response()
        }
    }
}

/// GET /api/v1/summary -- compact dashboard summary for the Summary page.
async fn api_summary(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let repos_guard = state.multi.repos.read().await;
    let target = match filter.repo {
        Some(ref s) => repos_guard.get(s).cloned(),
        None => repos_guard.values().next().cloned(),
    };
    drop(repos_guard);
    let ds = match target {
        Some(d) => d,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "no repo configured"})),
            )
                .into_response();
        }
    };

    let slug = format!("{}/{}", ds.config.repo_owner, ds.config.repo_name);
    let issues = ds.db.get_open_issues().unwrap_or_default();
    let untriaged = ds.db.get_untriaged_issues().unwrap_or_default();
    let prs = ds.db.get_open_pulls().unwrap_or_default();
    let conflicts = prs.iter().filter(|p| p.mergeable == Some(false)).count();

    // unanalyzed_prs: count PRs without an analysis row.
    let unanalyzed_count = prs
        .iter()
        .filter(|p| ds.db.get_pr_analysis(p.number).ok().flatten().is_none())
        .count();

    let now = chrono::Utc::now();
    let age_days = |s: &str| -> u32 {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| (now - d.with_timezone(&chrono::Utc)).num_days().max(0) as u32)
            .unwrap_or(0)
    };

    let to_issue_brief = |i: &crate::db::issues::Issue| {
        json!({
            "number": i.number,
            "title": i.title,
            "labels": i.labels,
            "age_days": age_days(&i.created_at),
        })
    };

    let to_pr_brief = |p: &crate::db::pulls::PullRequest, risk: Option<String>| {
        json!({
            "number": p.number,
            "title": p.title,
            "risk_level": risk,
            "ci_status": p.ci_status,
            "has_conflicts": p.mergeable == Some(false),
            "age_days": age_days(&p.created_at),
        })
    };

    // High-priority issues: any label starting with "priority:high" or "priority:critical".
    let high_priority_issues: Vec<_> = issues
        .iter()
        .filter(|i| {
            i.labels
                .iter()
                .any(|l| l.starts_with("priority:high") || l.starts_with("priority:critical"))
        })
        .take(5)
        .map(to_issue_brief)
        .collect();

    // Top issues: by reactions+1, then by recency.
    let mut sorted_issues = issues.clone();
    sorted_issues.sort_by(|a, b| {
        b.reactions_plus1
            .cmp(&a.reactions_plus1)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    let top_issues: Vec<_> = sorted_issues.iter().take(5).map(to_issue_brief).collect();

    // High-risk PRs: where analysis.risk_level is high or critical.
    let high_risk_prs: Vec<_> = prs
        .iter()
        .filter_map(|p| {
            ds.db.get_pr_analysis(p.number).ok().flatten().and_then(|a| {
                if a.risk_level == "high" || a.risk_level == "critical" {
                    Some(to_pr_brief(p, Some(a.risk_level)))
                } else {
                    None
                }
            })
        })
        .take(5)
        .collect();

    // Top PRs: oldest open first (these are the ones at risk of being forgotten).
    let mut sorted_prs = prs.clone();
    sorted_prs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let top_prs: Vec<_> = sorted_prs
        .iter()
        .take(5)
        .map(|p| {
            let risk = ds
                .db
                .get_pr_analysis(p.number)
                .ok()
                .flatten()
                .map(|a| a.risk_level);
            to_pr_brief(p, risk)
        })
        .collect();

    Json(json!({
        "repo": slug,
        "timestamp": now.to_rfc3339(),
        "open_issues": issues.len(),
        "untriaged_issues": untriaged.len(),
        "high_priority_issues": high_priority_issues,
        "top_issues": top_issues,
        "open_prs": prs.len(),
        "unanalyzed_prs": unanalyzed_count,
        "high_risk_prs": high_risk_prs,
        "top_prs": top_prs,
        "conflicts": conflicts,
    }))
    .into_response()
}

/// GET /api/v1/auth/status -- which credentials are detected.
async fn api_auth_status() -> impl IntoResponse {
    let creds = crate::login::load_credentials();

    let github = creds.contains_key("GITHUB_TOKEN")
        || std::env::var("GITHUB_TOKEN").is_ok()
        || std::env::var("WSHM_TOKEN").is_ok();

    let anthropic_kind = if creds.contains_key("ANTHROPIC_OAUTH_TOKEN")
        || std::env::var("ANTHROPIC_OAUTH_TOKEN").is_ok()
        || std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok()
    {
        Some("oauth")
    } else if creds.contains_key("ANTHROPIC_API_KEY") || std::env::var("ANTHROPIC_API_KEY").is_ok()
    {
        Some("api_key")
    } else {
        None
    };

    Json(json!({
        "github": github,
        "anthropic": anthropic_kind,
    }))
}

/// POST /api/v1/auth/github -- store GITHUB_TOKEN in .wshm/credentials.
async fn api_auth_github(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    let token = match body.get("token").and_then(|v| v.as_str()) {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "token is required"})),
            )
                .into_response();
        }
    };

    let mut creds = crate::login::load_credentials();
    creds.insert("GITHUB_TOKEN".to_string(), token);
    match crate::login::save_credentials(&creds) {
        Ok(()) => Json(json!({
            "status": "ok",
            "message": "GitHub token saved to .wshm/credentials",
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "message": format!("{e}")})),
        )
            .into_response(),
    }
}

/// POST /api/v1/auth/anthropic -- store Anthropic OAuth token or API key.
/// Body: {"token": "...", "kind": "oauth"|"api_key"}
async fn api_auth_anthropic(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    let token = match body.get("token").and_then(|v| v.as_str()) {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "token is required"})),
            )
                .into_response();
        }
    };
    let kind = body
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("oauth");

    let key = match kind {
        "oauth" => "ANTHROPIC_OAUTH_TOKEN",
        "api_key" => "ANTHROPIC_API_KEY",
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "kind must be 'oauth' or 'api_key'"})),
            )
                .into_response();
        }
    };

    let mut creds = crate::login::load_credentials();
    // Mutually exclusive: writing one removes the other so we don't
    // confuse resolve_anthropic_auth's priority.
    let other = if kind == "oauth" {
        "ANTHROPIC_API_KEY"
    } else {
        "ANTHROPIC_OAUTH_TOKEN"
    };
    creds.remove(other);
    creds.insert(key.to_string(), token);

    match crate::login::save_credentials(&creds) {
        Ok(()) => Json(json!({
            "status": "ok",
            "kind": kind,
            "message": format!("{key} saved to .wshm/credentials"),
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "message": format!("{e}")})),
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

/// GET /api/v1/logs -- tail of the daemon's in-memory log buffer.
///
/// Query: `tail` (default 200, max 5000), `level` (ERROR/WARN/INFO/DEBUG/TRACE),
/// `since` (numeric id — return only entries newer than this id).
async fn api_logs(
    State(state): State<Arc<WebState>>,
    Query(params): Query<LogsQuery>,
) -> impl IntoResponse {
    let logs = match state.logs.as_ref() {
        Some(b) => b,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "log buffer not configured"})),
            )
                .into_response();
        }
    };
    let tail = params
        .tail
        .map(|n| n.min(crate::daemon::log_buffer::MAX_ENTRIES))
        .or(Some(200));
    let level = params
        .level
        .as_deref()
        .and_then(crate::daemon::log_buffer::parse_level);
    let entries = logs.snapshot(tail, params.since, level).await;
    let last_id = entries.last().map(|e| e.id).or(params.since);
    Json(json!({
        "entries": entries,
        "last_id": last_id,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    tail: Option<usize>,
    level: Option<String>,
    since: Option<u64>,
}

/// OSS API routes sub-router (state-dependent).
///
/// Returns a `Router<Arc<WebState>>` containing only the OSS `/api/v1/*`
/// handlers. Useful for extension crates that want to assemble their own
/// final router from the OSS handlers plus their own extra handlers.
pub fn oss_api_routes() -> Router<Arc<WebState>> {
    Router::new()
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/issues", get(api_issues))
        .route("/api/v1/pulls", get(api_pulls))
        .route("/api/v1/triage", get(api_triage))
        .route("/api/v1/queue", get(api_queue))
        .route("/api/v1/activity", get(api_activity))
        .route("/api/v1/changelog", get(api_changelog))
        .route("/api/v1/revert/preview", get(api_revert_preview))
        .route("/api/v1/backups", get(api_list_backups))
        .route("/api/v1/backup", post(api_create_backup))
        .route("/api/v1/restore", post(api_restore_backup))
        .route("/api/v1/license", get(api_license))
        .route("/api/v1/license/activate", post(api_license_activate))
        .route("/api/v1/repos", get(api_list_repos).post(api_add_repo))
        .route("/api/v1/auth/status", get(api_auth_status))
        .route("/api/v1/auth/github", post(api_auth_github))
        .route("/api/v1/auth/anthropic", post(api_auth_anthropic))
        .route("/api/v1/auth/login", post(api_auth_login))
        .route("/api/v1/auth/logout", post(api_auth_logout))
        .route("/api/v1/auth/me", get(api_auth_me))
        .route("/api/v1/users", get(api_users_list).post(api_users_create))
        .route(
            "/api/v1/users/{id}",
            axum::routing::patch(api_users_update).delete(api_users_delete),
        )
        .route("/api/v1/logs", get(api_logs))
        .route("/api/v1/summary", get(api_summary))
}

/// Default SPA sub-router serving the embedded wshm-core web-dist.
pub fn default_spa_routes() -> Router<Arc<WebState>> {
    Router::new()
        .route("/", get(handle_spa_root))
        .fallback(handle_spa_fallback)
}

/// Basic-auth middleware wrapper exposed for extension crates.
///
/// Extension crates can use this to apply the same auth semantics when
/// building their own router with [`oss_api_routes`].
pub async fn auth_layer(state: State<Arc<WebState>>, req: Request<Body>, next: Next) -> Response {
    auth_middleware(state, req, next).await
}

/// Build the web UI router.  Merge this into the existing axum server or
/// use it standalone.
///
/// All `/api/v1/*` routes require basic auth (when a password is configured).
/// The `/health` endpoint is always public.
/// All other routes serve the embedded Svelte SPA.
pub fn web_routes(multi: Arc<MultiDaemonState>) -> Router {
    web_routes_with_extensions(multi, None, None, None, None)
}

/// Build the web UI router with optional extensions.
///
/// - `users`: enable RBAC mode by passing a populated UserStore. `None`
///   keeps the legacy single-credential `[web].username/password` flow.
/// - `extra_api`: additional `Router<Arc<WebState>>` whose routes get merged
///   into the main router under the same auth layer. Use this to register
///   Pro API endpoints from extension crates.
/// - `spa_override`: replaces the default embedded Svelte SPA router. Use
///   this to serve a different web-dist bundle (e.g. wshm-pro's full
///   OSS+Pro build).
///
/// The `WebState` is built from these inputs and shared with every sub-router.
pub fn web_routes_with_extensions(
    multi: Arc<MultiDaemonState>,
    users: Option<Arc<crate::auth::UserStore>>,
    logs: Option<Arc<crate::daemon::log_buffer::LogBuffer>>,
    extra_api: Option<Router<Arc<WebState>>>,
    spa_override: Option<Router<Arc<WebState>>>,
) -> Router {
    let state = Arc::new(WebState { multi, users, logs });

    let mut api_routes = oss_api_routes();
    if let Some(extra) = extra_api {
        api_routes = api_routes.merge(extra);
    }

    let spa_routes = spa_override.unwrap_or_else(default_spa_routes);

    Router::new()
        .merge(api_routes)
        .merge(spa_routes)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth_middleware,
        ))
        .with_state(state)
}
