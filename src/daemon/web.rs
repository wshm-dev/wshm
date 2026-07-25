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
    pub users: Option<Arc<dyn crate::auth::UserStoreBackend>>,
    /// Optional in-memory log buffer fed by the tracing layer. When `Some`,
    /// `GET /api/v1/logs` returns the daemon's recent log lines.
    pub logs: Option<Arc<crate::daemon::log_buffer::LogBuffer>>,
    /// Optional encrypted secret store backing the Settings → Secrets tab.
    /// When `Some`, the `/api/v1/secrets/*` routes are functional. Using a
    /// trait object so OSS (SQLite) and Pro (Postgres) can plug their own
    /// backend in.
    pub secrets: Option<Arc<dyn crate::secrets::SecretStore>>,
}

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RepoFilter {
    repo: Option<String>,
}

/// Combined query string for paginated list endpoints
/// (`/issues`, `/pulls`, `/triage`, `/queue`, `/activity`).
///
/// `limit` is clamped to [1, 500] with a default of 50; `offset` defaults to 0.
#[derive(Debug, Deserialize)]
struct ListQuery {
    repo: Option<String>,
    #[allow(dead_code)] // accepted for forward-compat; current endpoints only return open items
    state: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    /// Sort field. Default: `updated_at` (descending). Pass `score` to
    /// rank by the issue/PR scoring algorithm (see `[scoring]` config).
    sort: Option<String>,
}

const PAGE_DEFAULT_LIMIT: usize = 50;
const PAGE_MAX_LIMIT: usize = 500;

/// Slice the aggregated list into a `{items, total, limit, offset}` page.
/// `items` should already be sorted in the caller's preferred display order.
fn paginate<T: serde::Serialize>(
    items: Vec<T>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> serde_json::Value {
    let total = items.len();
    let limit = limit.unwrap_or(PAGE_DEFAULT_LIMIT).clamp(1, PAGE_MAX_LIMIT);
    let offset = offset.unwrap_or(0).min(total);
    let slice: Vec<T> = items.into_iter().skip(offset).take(limit).collect();
    json!({
        "items": slice,
        "total": total,
        "limit": limit,
        "offset": offset,
    })
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
    let mut mac =
        Hmac::<Sha256>::new_from_slice(password.as_bytes()).expect("HMAC accepts any key size");
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
    let mut mac =
        Hmac::<Sha256>::new_from_slice(password.as_bytes()).expect("HMAC accepts any key size");
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

/// HMAC signing key for the user-id session cookie.
///
/// Resolution order:
///   1. `WSHM_JWT_SECRET` env var (recommended for production — survives
///      restarts so sessions stay valid across deploys).
///   2. A 32-byte random key generated once per process on first use.
///      Sessions are invalidated on restart but cannot be forged offline.
///
/// We deliberately removed the previous string fallback (`wshm-cookie-
/// fallback`): once published in source it became a public HMAC key,
/// letting anyone mint a cookie for any user_id on a deploy that forgot
/// to set `WSHM_JWT_SECRET`.
fn user_cookie_secret() -> String {
    use std::sync::OnceLock;
    static EPHEMERAL: OnceLock<String> = OnceLock::new();
    if let Ok(v) = std::env::var("WSHM_JWT_SECRET") {
        if !v.trim().is_empty() {
            return v;
        }
    }
    EPHEMERAL
        .get_or_init(|| {
            use rand::RngCore;
            let mut buf = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut buf);
            tracing::warn!(
                "WSHM_JWT_SECRET not set — using ephemeral session key; \
                 sessions will not survive a daemon restart. \
                 Set WSHM_JWT_SECRET to a 32-byte hex string for stable sessions."
            );
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
        })
        .clone()
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
/// Optional defense-in-depth for the `WSHM_TRUST_PROXY_AUTH` trust model.
///
/// `WSHM_TRUST_PROXY_AUTH=1` makes the daemon trust forwarded-identity headers
/// (`X-Forwarded-User` / `X-Forwarded-Email` / `X-Auth-Request-Email`) set by
/// an upstream oauth2-proxy. That trust is only safe if the daemon listener is
/// network-isolated so ONLY the proxy can reach it — otherwise any client that
/// can connect directly (a sidecar, SSRF, direct pod/port access) can forge
/// those headers and authenticate as any email.
///
/// When `WSHM_PROXY_AUTH_TOKEN` is set, the proxy must additionally forward a
/// matching `X-Wshm-Proxy-Token` header (compared in constant time), so mere
/// header presence is no longer sufficient. A dedicated header name (rather than
/// the `X-Auth-Request-*` family) avoids any collision with headers oauth2-proxy
/// sets/strips itself, so the shared secret survives the proxy hop intact.
///
/// Fail-closed by default: if `WSHM_TRUST_PROXY_AUTH` is enabled but no shared
/// secret is configured, forwarded-identity headers are REJECTED. Trusting
/// those headers with no shared secret is only safe behind a network-isolated
/// listener, so relying on isolation alone must be acknowledged explicitly via
/// `WSHM_TRUST_PROXY_AUTH_NO_TOKEN=1` — otherwise any client that can reach the
/// listener directly could forge an identity (and self-promote to admin when
/// the forged email matches `WSHM_ADMIN_EMAIL`).
fn proxy_shared_secret_ok(headers: &HeaderMap) -> bool {
    let expected = std::env::var("WSHM_PROXY_AUTH_TOKEN").unwrap_or_default();
    if expected.is_empty() {
        // No shared secret configured. Fail closed unless the operator has
        // explicitly acknowledged that the trusted listener is network
        // isolated (only the upstream proxy can reach it).
        return std::env::var("WSHM_TRUST_PROXY_AUTH_NO_TOKEN")
            .ok()
            .filter(|v| v == "1" || v == "true")
            .is_some();
    }
    let presented = headers
        .get("x-wshm-proxy-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    ct_eq(presented.as_bytes(), expected.as_bytes())
}

async fn current_user(state: &Arc<WebState>, headers: &HeaderMap) -> Option<crate::auth::User> {
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
    if trusts_proxy && proxy_shared_secret_ok(headers) {
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
/// Returns Err(response) if the request is a mutating /api/v1/ call that
/// lacks a CSRF-defeating custom header. Browsers cannot set custom
/// headers on cross-origin requests without a CORS preflight, which we
/// never grant — so requiring the header on POST/PATCH/PUT/DELETE blocks
/// classic CSRF (form-submit / fetch with credentials:include from a
/// third-party site, including sibling subdomains under the shared
/// `.cloud.rtk-ai.app` cookie domain).
///
/// Only the unauthenticated bootstrap endpoints (login, logout) are exempt —
/// they have no authenticated state to abuse. `license/activate` is an
/// authenticated, state-changing endpoint, so it must carry the CSRF header
/// like every other mutating route.
#[allow(clippy::result_large_err)] // axum Response is the natural error shape here
fn csrf_check(req: &Request<Body>) -> Result<(), Response> {
    let method = req.method();
    if !matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        return Ok(());
    }
    let path = req.uri().path();
    if !path.starts_with("/api/v1/") {
        return Ok(());
    }
    if matches!(path, "/api/v1/auth/login" | "/api/v1/auth/logout") {
        return Ok(());
    }
    let has_token = req
        .headers()
        .get("x-wshm-csrf")
        .or_else(|| req.headers().get("x-requested-with"))
        .is_some();
    if has_token {
        return Ok(());
    }
    Err((
        StatusCode::FORBIDDEN,
        Json(json!({
            "error": "CSRF protection: include `X-Wshm-Csrf: 1` (or `X-Requested-With`) on POST/PATCH/PUT/DELETE"
        })),
    )
        .into_response())
}

/// Build the "not authenticated" response for a rejected request.
///
/// Browsers (and SPA fetches) get a 302 to `/login`; API/curl callers get a
/// 401 (with a `WWW-Authenticate: Basic` challenge for non-browser clients
/// so CLI tooling can retry with Basic Auth).
fn unauthorized_response(path: &str, headers: &HeaderMap) -> Response {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let wants_json = path.starts_with("/api/") || accept.contains("application/json");
    let is_browser_fetch = headers.contains_key("sec-fetch-mode")
        || headers.contains_key("sec-fetch-site")
        || headers.contains_key("origin");

    if wants_json {
        if is_browser_fetch {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthorized"})),
            )
                .into_response()
        } else {
            (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Basic realm=\"wshm\"")],
                Json(json!({"error": "unauthorized"})),
            )
                .into_response()
        }
    } else {
        (
            StatusCode::FOUND,
            [(header::LOCATION, HeaderValue::from_static("/login"))],
        )
            .into_response()
    }
}

/// Whether running completely without configured credentials is explicitly
/// permitted. Without this opt-in, a daemon that has neither RBAC users nor
/// a `[web].password` denies all non-public requests (deny-by-default)
/// rather than serving every repo's data anonymously.
fn auth_explicitly_disabled() -> bool {
    std::env::var("WSHM_AUTH_DISABLED")
        .ok()
        .filter(|v| v == "1" || v == "true")
        .is_some()
}

/// Gather every repo's configured `[web]` (username, password) credential.
async fn gather_web_cfgs(state: &Arc<WebState>) -> Vec<(String, String)> {
    let repos = state.multi.repos.read().await;
    repos
        .values()
        .filter_map(|ds| {
            ds.config
                .web
                .password
                .as_ref()
                .map(|p| (ds.config.web.username.clone(), p.clone()))
        })
        .collect()
}

/// Constant-time Basic Auth check against any configured `[web]` credential.
/// A match is a real authenticated request (a valid shared password), not an
/// anonymous one, so honoring it does not reopen the fail-open hole.
fn basic_auth_matches(headers: &HeaderMap, web_cfgs: &[(String, String)]) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|decoded| {
            if let Some((user, pass)) = decoded.split_once(':') {
                web_cfgs.iter().any(|(u, p)| {
                    ct_eq(user.as_bytes(), u.as_bytes()) & ct_eq(pass.as_bytes(), p.as_bytes())
                })
            } else {
                false
            }
        })
        .unwrap_or(false)
}

async fn auth_middleware(
    State(state): State<Arc<WebState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    if is_public_path(&path) {
        return next.run(req).await;
    }
    if let Err(e) = csrf_check(&req) {
        return e;
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
        // RBAC is active but the request carries no valid identity. Do NOT
        // fall through to the legacy "no password configured" branch (that
        // used to serve all data unauthenticated even with RBAC on). But
        // still honor the documented Basic Auth escape hatch
        // (`WSHM_WEB_PASSWORD` / `[web].password`) used by CLI/curl and the
        // dev proxy: a valid shared password is authenticated, not anonymous.
        // Such a request carries no `User`, so role-gated handlers
        // (require_min_role) still demand a real RBAC login.
        let web_cfgs = gather_web_cfgs(&state).await;
        if !web_cfgs.is_empty() && basic_auth_matches(req.headers(), &web_cfgs) {
            req.extensions_mut()
                .insert(None as Option<crate::auth::User>);
            return next.run(req).await;
        }
        return unauthorized_response(&path, req.headers());
    }
    req.extensions_mut()
        .insert(None as Option<crate::auth::User>);

    // Multi-repo: gather every repo's [web] credential. The previous
    // code picked an arbitrary repo via `repos.values().next()`, which
    // made auth non-deterministic when two repos had different
    // `[web].password` values — the gate behaved as one or the other
    // depending on HashMap iteration order.
    //
    // New behavior: a Basic Auth pair / signed cookie is accepted when
    // it matches ANY configured repo's [web] block. Empty credential
    // lists (no [web] anywhere) keep "auth disabled" semantics.
    let web_cfgs = gather_web_cfgs(&state).await;

    // No credentials configured anywhere. Deny by default — serving every
    // repo's data to anonymous callers must be an explicit opt-in
    // (WSHM_AUTH_DISABLED=1), not the silent fallback it used to be.
    if web_cfgs.is_empty() {
        if auth_explicitly_disabled() {
            return next.run(req).await;
        }
        return unauthorized_response(&path, req.headers());
    }
    let valid_passwords: Vec<String> = web_cfgs.iter().map(|(_, p)| p.clone()).collect();

    // 1) Trust oauth2-proxy headers when explicitly enabled. Looking for
    //    X-Forwarded-User / X-Forwarded-Email / X-Auth-Request-Email which
    //    oauth2-proxy sets after a successful SSO.
    if std::env::var("WSHM_TRUST_PROXY_AUTH")
        .ok()
        .filter(|v| v == "1" || v == "true")
        .is_some()
        && proxy_shared_secret_ok(req.headers())
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
    //    already tried above via current_user(). The cookie is HMAC-signed
    //    with one of the configured passwords; accept any match across
    //    repos (rotation of one repo's password doesn't invalidate the
    //    other's outstanding sessions).
    if state.users.is_none() {
        if let Some(cookie_val) = read_cookie(req.headers(), "wshm_session") {
            if valid_passwords
                .iter()
                .any(|p| verify_session_cookie(p, cookie_val))
            {
                return next.run(req).await;
            }
        }
    }

    // 3) Basic Auth fallback (CLI / curl). Match against ANY configured
    //    repo's (username, password) pair so a single CI script can hit
    //    a multi-repo daemon without picking the "right" repo's creds.
    if basic_auth_matches(req.headers(), &web_cfgs) {
        return next.run(req).await;
    }

    unauthorized_response(&path, req.headers())
}

/// Constant-time byte-slice equality. Returns `true` only when both slices
/// have the same length and identical contents, without short-circuiting on
/// the first differing byte (no timing oracle on the shared secret).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Length is not itself secret here (it leaks nothing about the
        // matching prefix), and comparing different-length inputs as equal
        // would be wrong. Diverge only on length.
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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
                // No such user, or a user with no local password hash (SSO-
                // only). Verify the supplied password against a fixed dummy
                // hash so this path pays the same argon2 cost as a real wrong
                // password — closing the username-enumeration timing oracle.
                crate::auth::verify_password(&password, crate::auth::dummy_password_hash());
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

    // Constant-time comparison so the static shared secret can't be
    // recovered character-by-character via response timing. Both checks
    // always run (no short-circuit) via the non-short-circuiting `&`.
    let creds_ok = ct_eq(username.as_bytes(), web_cfg.username.as_bytes())
        & ct_eq(password.as_bytes(), required_password.as_bytes());
    if !creds_ok {
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

    // Only reflect forwarded-identity headers when the daemon is configured to
    // trust the upstream proxy AND the shared-secret gate passes — matching the
    // guard used in current_user(). Otherwise an attacker who can reach the
    // listener could make the UI display a forged identity string.
    let trusts_proxy = std::env::var("WSHM_TRUST_PROXY_AUTH")
        .ok()
        .filter(|v| v == "1" || v == "true")
        .is_some();
    if trusts_proxy && proxy_shared_secret_ok(&headers) {
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
#[allow(clippy::result_large_err)] // axum Response is the natural error shape here
fn require_admin(
    user: &axum::Extension<Option<crate::auth::User>>,
) -> Result<crate::auth::User, Response> {
    require_min_role(user, crate::auth::Role::Admin)
}

/// Generic role gate. Returns the User when its role is at least `min`,
/// otherwise a 401/403 response. Routes that don't require any specific
/// minimum still go through the auth middleware so we always have an
/// authenticated user when this helper is called.
#[allow(clippy::result_large_err)] // axum Response is the natural error shape here
fn require_min_role(
    user: &axum::Extension<Option<crate::auth::User>>,
    min: crate::auth::Role,
) -> Result<crate::auth::User, Response> {
    match &user.0 {
        Some(u) if u.role.has_at_least(min) => Ok(u.clone()),
        Some(_) => Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!("{} role or higher required", min.as_str())})),
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

    // Snapshot the repo map and drop the read guard before the per-repo
    // blocking DB work, so the RwLock is not held across the whole scan
    // (which would block repos.write(): repo add/remove/config reload).
    let __repos_snapshot: Vec<(String, Arc<super::DaemonState>)> = {
        let g = state.multi.repos.read().await;
        g.iter().map(|(s, d)| (s.clone(), Arc::clone(d))).collect()
    };
    for (slug, ds) in __repos_snapshot.iter() {
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
            apply: ds.apply(),
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
/// Paginated: returns `{items, total, limit, offset}`. Items are sorted by
/// `updated_at` desc (newest first), tiebreaker `number` desc.
async fn api_issues(
    State(state): State<Arc<WebState>>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let mut all_issues = Vec::new();

    // Snapshot the repo map and drop the read guard before the per-repo
    // blocking DB work, so the RwLock is not held across the whole scan
    // (which would block repos.write(): repo add/remove/config reload).
    let __repos_snapshot: Vec<(String, Arc<super::DaemonState>)> = {
        let g = state.multi.repos.read().await;
        g.iter().map(|(s, d)| (s.clone(), Arc::clone(d))).collect()
    };
    for (slug, ds) in __repos_snapshot.iter() {
        if let Some(ref f) = q.repo {
            if f != slug {
                continue;
            }
        }
        if let Ok(issues) = ds.db.get_open_issues() {
            // Batch-load all triage results once (avoids an N+1 per-issue
            // get_triage_result query).
            let triage_results = ds.db.get_all_triage_results().unwrap_or_default();
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
                let triage = triage_results.get(&issue.number).cloned();
                let (score, score_breakdown) = crate::pipelines::pr_health::score_issue_with(
                    &issue,
                    triage.as_ref(),
                    &ds.config.scoring.issue,
                );
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
                    "confidence": triage.as_ref().map(|t| t.confidence),
                    "category": triage.as_ref().map(|t| t.category.as_str()),
                    "domains": triage.as_ref().map(|t| &t.domains),
                    "score": score,
                    "score_breakdown": score_breakdown,
                    "pr_status": pr_status,
                    "linked_prs": linked,
                    "url": crate::git_provider::web_url_for_issue(&ds.config, issue.number),
                }));
            }
        }
    }

    let sort_by_score = q.sort.as_deref() == Some("score");
    if sort_by_score {
        all_issues.sort_by(|a, b| {
            let sa = a["score"].as_i64().unwrap_or(i64::MIN);
            let sb = b["score"].as_i64().unwrap_or(i64::MIN);
            sb.cmp(&sa).then_with(|| {
                let na = a["number"].as_u64().unwrap_or(0);
                let nb = b["number"].as_u64().unwrap_or(0);
                nb.cmp(&na)
            })
        });
    } else {
        all_issues.sort_by(|a, b| {
            let ka = a["updated_at"].as_str().unwrap_or("");
            let kb = b["updated_at"].as_str().unwrap_or("");
            kb.cmp(ka).then_with(|| {
                let na = a["number"].as_u64().unwrap_or(0);
                let nb = b["number"].as_u64().unwrap_or(0);
                nb.cmp(&na)
            })
        });
    }

    Json(paginate(all_issues, q.limit, q.offset))
}

/// GET /api/v1/pulls -- open PRs from DB.
/// Paginated: returns `{items, total, limit, offset}`. Items are sorted by
/// `updated_at` desc (newest first), tiebreaker `number` desc.
async fn api_pulls(
    State(state): State<Arc<WebState>>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let mut all_prs = Vec::new();

    // Snapshot the repo map and drop the read guard before the per-repo
    // blocking DB work, so the RwLock is not held across the whole scan
    // (which would block repos.write(): repo add/remove/config reload).
    let __repos_snapshot: Vec<(String, Arc<super::DaemonState>)> = {
        let g = state.multi.repos.read().await;
        g.iter().map(|(s, d)| (s.clone(), Arc::clone(d))).collect()
    };
    for (slug, ds) in __repos_snapshot.iter() {
        if let Some(ref f) = q.repo {
            if f != slug {
                continue;
            }
        }
        if let Ok(prs) = ds.db.get_open_pulls() {
            let analyses = ds.db.get_all_pr_analyses().unwrap_or_default();
            for pr in prs {
                let analysis = analyses.get(&pr.number);
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
                    "review_decision": pr.review_decision,
                    "review_decision_at": pr.review_decision_at,
                    "risk_level": analysis.map(|a| a.risk_level.as_str()),
                    "pr_type": analysis.map(|a| a.pr_type.as_str()),
                    "summary": analysis.map(|a| a.summary.as_str()),
                    "domains": analysis.map(|a| &a.domains),
                    "url": crate::git_provider::web_url_for_pr(&ds.config, pr.number),
                }));
            }
        }
    }

    all_prs.sort_by(|a, b| {
        let ka = a["updated_at"].as_str().unwrap_or("");
        let kb = b["updated_at"].as_str().unwrap_or("");
        kb.cmp(ka).then_with(|| {
            let na = a["number"].as_u64().unwrap_or(0);
            let nb = b["number"].as_u64().unwrap_or(0);
            nb.cmp(&na)
        })
    });

    Json(paginate(all_prs, q.limit, q.offset))
}

/// GET /api/v1/triage -- recent triage results.
/// Paginated: returns `{items, total, limit, offset}` sorted by `acted_at` desc.
async fn api_triage(
    State(state): State<Arc<WebState>>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let mut all_results = Vec::new();

    // Snapshot the repo map and drop the read guard before the per-repo
    // blocking DB work, so the RwLock is not held across the whole scan
    // (which would block repos.write(): repo add/remove/config reload).
    let __repos_snapshot: Vec<(String, Arc<super::DaemonState>)> = {
        let g = state.multi.repos.read().await;
        g.iter().map(|(s, d)| (s.clone(), Arc::clone(d))).collect()
    };
    for (slug, ds) in __repos_snapshot.iter() {
        if let Some(ref f) = q.repo {
            if f != slug {
                continue;
            }
        }
        // Pull a generous slice from each repo so cross-repo merge has enough
        // material; final cut happens after sort + paginate.
        if let Ok(results) = ds.db.recent_activity(PAGE_MAX_LIMIT) {
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

    all_results.sort_by(|a, b| {
        let ka = a["acted_at"].as_str().unwrap_or("");
        let kb = b["acted_at"].as_str().unwrap_or("");
        kb.cmp(ka)
    });

    Json(paginate(all_results, q.limit, q.offset))
}

/// GET /api/v1/queue -- merge queue: open PRs with basic scoring data.
/// Paginated: returns `{items, total, limit, offset}` sorted by `score` desc.
async fn api_queue(
    State(state): State<Arc<WebState>>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let mut queue = Vec::new();

    // Snapshot the repo map and drop the read guard before the per-repo
    // blocking DB work, so the RwLock is not held across the whole scan
    // (which would block repos.write(): repo add/remove/config reload).
    let __repos_snapshot: Vec<(String, Arc<super::DaemonState>)> = {
        let g = state.multi.repos.read().await;
        g.iter().map(|(s, d)| (s.clone(), Arc::clone(d))).collect()
    };
    for (slug, ds) in __repos_snapshot.iter() {
        if let Some(ref f) = q.repo {
            if f != slug {
                continue;
            }
        }
        if let Ok(prs) = ds.db.get_open_pulls() {
            let analyses = ds.db.get_all_pr_analyses().unwrap_or_default();
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
                let analysis = analyses.get(&pr.number);
                if let Some(a) = analysis {
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
                    "risk_level": analysis.map(|a| &a.risk_level),
                    "pr_type": analysis.map(|a| &a.pr_type),
                    "created_at": pr.created_at,
                }));
            }
        }
    }

    // Sort descending by score, tiebreaker number desc
    queue.sort_by(|a, b| {
        let sa = a.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        let sb = b.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        sb.cmp(&sa).then_with(|| {
            let na = a["number"].as_u64().unwrap_or(0);
            let nb = b["number"].as_u64().unwrap_or(0);
            nb.cmp(&na)
        })
    });

    Json(paginate(queue, q.limit, q.offset))
}

/// GET /api/v1/activity -- combined recent triage + PR analysis activity.
/// Paginated: returns `{items, total, limit, offset}` sorted by `at` desc.
async fn api_activity(
    State(state): State<Arc<WebState>>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let mut entries: Vec<ActivityEntry> = Vec::new();

    // Snapshot the repo map and drop the read guard before the per-repo
    // blocking DB work, so the RwLock is not held across the whole scan
    // (which would block repos.write(): repo add/remove/config reload).
    let __repos_snapshot: Vec<(String, Arc<super::DaemonState>)> = {
        let g = state.multi.repos.read().await;
        g.iter().map(|(s, d)| (s.clone(), Arc::clone(d))).collect()
    };
    for (slug, ds) in __repos_snapshot.iter() {
        if let Some(ref f) = q.repo {
            if f != slug {
                continue;
            }
        }

        // Pull a generous slice from each repo so cross-repo merge has enough
        // material; final cut happens after sort + paginate.
        if let Ok(results) = ds.db.recent_activity(PAGE_MAX_LIMIT) {
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

        // PR analysis activity -- one batched query returns every analysis
        // row directly (only analyzed PRs produce an ActivityEntry).
        if let Ok(analyses) = ds.db.get_all_pr_analyses() {
            for (pr_number, a) in analyses {
                entries.push(ActivityEntry {
                    entry_type: "pr_analysis".to_string(),
                    repo: slug.clone(),
                    number: pr_number,
                    summary: Some(a.summary),
                    category: Some(a.pr_type),
                    risk_level: Some(a.risk_level),
                    at: a.analyzed_at,
                });
            }
        }
    }

    // Sort by timestamp descending
    entries.sort_by(|a, b| b.at.cmp(&a.at));

    Json(paginate(entries, q.limit, q.offset))
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

    // Snapshot the repo map and drop the read guard before the per-repo
    // blocking DB work, so the RwLock is not held across the whole scan
    // (which would block repos.write(): repo add/remove/config reload).
    let __repos_snapshot: Vec<(String, Arc<super::DaemonState>)> = {
        let g = state.multi.repos.read().await;
        g.iter().map(|(s, d)| (s.clone(), Arc::clone(d))).collect()
    };
    for (slug, ds) in __repos_snapshot.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }

        let closed_prs = ds.db.get_closed_pulls(100).map(|pulls| {
            pulls
                .into_iter()
                .map(|pr| {
                    json!({
                        "number": pr.number,
                        "title": pr.title,
                        "labels": pr.labels,
                        "author": pr.author,
                        "merged_at": pr.updated_at,
                    })
                })
                .collect::<Vec<_>>()
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

    // Snapshot the repo map and drop the read guard before the per-repo
    // blocking DB work, so the RwLock is not held across the whole scan
    // (which would block repos.write(): repo add/remove/config reload).
    let __repos_snapshot: Vec<(String, Arc<super::DaemonState>)> = {
        let g = state.multi.repos.read().await;
        g.iter().map(|(s, d)| (s.clone(), Arc::clone(d))).collect()
    };
    for (slug, ds) in __repos_snapshot.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }

        let mut comments_count = 0usize;
        let mut labels_count = 0usize;

        // Batch-load per-issue applied-label counts once (avoids one
        // get_wshm_applied_labels query per open issue).
        if let Ok(issues) = ds.db.get_open_issues() {
            let label_counts = ds.db.get_applied_label_counts().unwrap_or_default();
            for issue in &issues {
                labels_count += label_counts.get(&issue.number).copied().unwrap_or(0);
            }
        }

        if let Ok(results) = ds.db.recent_activity(1000) {
            comments_count = results.len();
        }

        // Count open PRs that have an analysis row using the batched map
        // instead of a per-PR get_pr_analysis query.
        let analyses = ds.db.get_all_pr_analyses().unwrap_or_default();
        let pr_analyses_count = ds
            .db
            .get_open_pulls()
            .unwrap_or_default()
            .iter()
            .filter(|pr| analyses.contains_key(&pr.number))
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

/// POST /api/v1/backup -- create a backup. Requires `operator` role.
async fn api_create_backup(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_min_role(&user, crate::auth::Role::Operator) {
            return e;
        }
    }
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

/// POST /api/v1/sync/incremental?repo=slug -- run incremental GitHub sync
/// for a single repo, or all configured repos when `repo` is omitted.
/// Requires `member` role or higher.
async fn api_sync_incremental(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Query(filter): Query<RepoFilter>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_min_role(&user, crate::auth::Role::Member) {
            return e;
        }
    }
    run_sync(&state, filter.repo.as_deref(), false).await
}

/// POST /api/v1/sync/full?repo=slug -- run full GitHub sync. Same scoping
/// rule as the incremental variant. Requires `operator` role or higher
/// because a full sync can drain GitHub rate limit.
async fn api_sync_full(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Query(filter): Query<RepoFilter>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_min_role(&user, crate::auth::Role::Operator) {
            return e;
        }
    }
    run_sync(&state, filter.repo.as_deref(), true).await
}

async fn run_sync(state: &WebState, repo_filter: Option<&str>, full: bool) -> Response {
    let repos_guard = state.multi.repos.read().await;
    let targets: Vec<(String, Arc<super::DaemonState>)> = match repo_filter {
        Some(slug) => repos_guard
            .get(slug)
            .map(|d| vec![(slug.to_string(), Arc::clone(d))])
            .unwrap_or_default(),
        None => repos_guard
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect(),
    };
    drop(repos_guard);

    if targets.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "status": "error",
                "synced": Vec::<String>::new(),
                "errors": [{"repo": repo_filter.unwrap_or("(all)"), "error": "no matching repo configured"}],
            })),
        )
            .into_response();
    }

    // Sync repos concurrently with bounded fan-out so an all-repos sync
    // does not run strictly serially. Keep the concurrency modest to respect
    // GitHub rate limits (mirrors github/sync.rs).
    const SYNC_CONCURRENCY: usize = 4;
    use futures::stream::StreamExt;
    let results: Vec<(String, Result<(), anyhow::Error>)> = futures::stream::iter(targets)
        .map(|(slug, daemon)| async move {
            let result = if full {
                crate::github::sync::full_sync(&daemon.gh(), daemon.db.as_ref()).await
            } else {
                crate::github::sync::incremental_sync_full(&daemon.gh(), daemon.db.as_ref()).await
            };
            (slug, result)
        })
        .buffer_unordered(SYNC_CONCURRENCY)
        .collect()
        .await;

    let mut synced = Vec::new();
    let mut errors = Vec::new();
    for (slug, result) in results {
        match result {
            Ok(()) => synced.push(slug),
            Err(e) => {
                // Log the full chain server-side; return a sanitized
                // message in the HTTP body to avoid leaking internal paths.
                tracing::warn!("[{slug}] sync failed: {e:#}");
                errors.push(json!({"repo": slug, "error": "sync failed; see server logs"}));
            }
        }
    }

    Json(json!({
        "status": if errors.is_empty() { "ok" } else { "partial" },
        "synced": synced,
        "errors": errors,
    }))
    .into_response()
}

/// POST /api/v1/restore -- restore from backup. Requires `operator` role.
async fn api_restore_backup(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_min_role(&user, crate::auth::Role::Operator) {
            return e;
        }
    }
    let requested = match body.get("path").and_then(|v| v.as_str()) {
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

    // Restore is a privileged operation regardless of RBAC mode: it can
    // overwrite the database, config, and credentials. Constrain the source
    // to the .wshm/ backups directory so a single-password session cannot
    // restore from an arbitrary filesystem path (which, combined with the
    // tarball it points at, would otherwise grant arbitrary file write).
    let backups_dir = std::path::PathBuf::from(".wshm");
    let candidate = std::path::Path::new(&requested);
    // Only accept a bare backup file name (no separators, no parent refs).
    let file_name = candidate.file_name().and_then(|n| n.to_str());
    let is_bare_name = file_name == Some(requested.as_str());
    if !is_bare_name
        || requested.contains("..")
        || candidate.is_absolute()
        || file_name.is_none_or(|n| !n.starts_with("backup-") || !n.ends_with(".tar.gz"))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "error",
                "message": "path must be a backup file name inside the .wshm/ directory",
            })),
        )
            .into_response();
    }
    let resolved = backups_dir.join(file_name.unwrap());

    let args = crate::cli::RestoreArgs {
        file: resolved.to_string_lossy().to_string(),
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
        // Search ships in every Pro Docker image; the route file
        // doesn't exist in OSS builds, so route-presence is itself the
        // gate. We only check `is_pro` here so legacy JWTs (issued
        // before "search" was added to the wshm-api features list) get
        // the feature too — re-activation just adds the explicit
        // entry, no behavior change.
        ("search", "Global full-text search", is_pro),
        // PR Insights is a Pro-only feature: its route and page live
        // in wshm-pro, not here. Declared in this list so the SPA can
        // gate the sidebar entry; `is_pro` is the enabled flag.
        ("pr-insights", "PR insights dashboard", is_pro),
        // Issue Insights — like pr-insights, the route and page live in
        // wshm-pro. Declared here only so the SPA can gate the sidebar
        // entry; `is_pro` is the enabled flag.
        ("issue-insights", "Issue insights dashboard", is_pro),
        // Usage dashboard (Pro). Tracks LLM invocations from the Pro
        // Postgres backend + live GitHub rate limit. Sidebar entry is
        // gated; the page itself ships only in the Pro web bundle.
        ("usage-dashboard", "LLM + API usage dashboard", is_pro),
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
        // Version of the running binary. Pro registers its own
        // CARGO_PKG_VERSION via `pro_hooks::set_pro_version` at startup
        // so the SPA can show "0.31.1-pro" rather than the wshm-core
        // path-dep version.
        "version": crate::pro_hooks::pro_version()
            .unwrap_or(env!("CARGO_PKG_VERSION")),
    }))
}

/// POST /api/v1/license -- activate a license key from the web UI.
/// Requires `admin` role.
async fn api_license_activate(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_admin(&user) {
            return e;
        }
    }
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
        let plan = body["license"]["type"]
            .as_str()
            .unwrap_or("pro")
            .to_string();

        // Persist to DB first (authoritative store — survives PVC file
        // truncation that historically wiped license.jwt). Then mirror
        // to the legacy file path for rétrocompat with operators that
        // still read it directly.
        let db_path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".wshm")
            .join("state.db");
        if let Ok(conn) = crate::db::licenses::open_state_db(&db_path) {
            if let Err(e) =
                crate::db::licenses::store(&conn, token, Some(key), Some(&plan), None, None)
            {
                tracing::warn!("Failed to persist license to state.db: {e}");
            }
        }

        let path = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".wshm")
            .join("license.jwt");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, token) {
            tracing::warn!("Failed to mirror license to {}: {e}", path.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            {
                // A failure here can leave the license JWT world-readable
                // (default umask). The DB copy is authoritative, but warn so
                // an operator can tighten the on-disk file's permissions.
                tracing::warn!(
                    "Failed to set 0o600 on license file {}: {e}",
                    path.display()
                );
            }
        }

        Ok(plan)
    } else {
        Err(body["error"]
            .as_str()
            .unwrap_or("Activation failed")
            .to_string())
    }
}

/// GET /api/v1/repos -- list configured repos with their feature flags.
async fn api_list_repos(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    let repos = state.multi.repos.read().await;
    let list: Vec<serde_json::Value> = repos
        .iter()
        .map(|(slug, ds)| {
            json!({
                "slug": slug,
                "apply": ds.apply(),
                "wshm_dir": ds.config.wshm_dir.display().to_string(),
                "features": ds.features(),
            })
        })
        .collect();
    let dynamic = state.multi.runtime.is_some();
    Json(json!({ "repos": list, "dynamic_add_supported": dynamic }))
}

/// GET /api/v1/repos/{slug}/features -- read the per-repo feature toggles.
/// Includes `apply` (master dry-run/apply toggle) on top of the standard
/// RepoFeatures fields, so the Settings UI can render a single modal.
async fn api_repo_features_get(
    State(state): State<Arc<WebState>>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> Response {
    let repos = state.multi.repos.read().await;
    match repos.get(&slug) {
        Some(ds) => {
            let mut resp = json!(ds.features());
            if let Some(obj) = resp.as_object_mut() {
                obj.insert("apply".to_string(), json!(ds.apply()));
            }
            Json(resp).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("repo '{slug}' not configured")})),
        )
            .into_response(),
    }
}

/// PATCH /api/v1/repos/{slug}/features -- update the per-repo feature
/// toggles. Body is a partial RepoFeatures JSON (any subset of fields).
/// Persists to ~/.wshm/global.toml AND swaps the in-memory state so the
/// change takes effect on the next pipeline iteration.
async fn api_repo_features_patch(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_admin(&user) {
            return e;
        }
    }

    let repos = state.multi.repos.read().await;
    let ds = match repos.get(&slug) {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("repo '{slug}' not configured")})),
            )
                .into_response();
        }
    };
    drop(repos);

    // Merge partial update over current snapshot.
    let mut features = ds.features();
    macro_rules! patch_bool {
        ($field:ident) => {
            if let Some(v) = body.get(stringify!($field)).and_then(|v| v.as_bool()) {
                features.$field = v;
            }
        };
    }
    patch_bool!(collect_issues);
    patch_bool!(collect_prs);
    patch_bool!(triage_issues);
    patch_bool!(analyze_prs);
    patch_bool!(review_prs);
    patch_bool!(auto_pr);
    patch_bool!(auto_merge);

    // Filters: full replace if a `filters` object is in the body.
    if let Some(f_body) = body.get("filters") {
        if let Ok(parsed) = serde_json::from_value::<crate::config::RepoFilters>(f_body.clone()) {
            features.filters = parsed;
        }
    }

    // Apply to in-memory state immediately.
    ds.set_features(features.clone());

    // Master apply-mode toggle. Lives on RepoEntry, not RepoFeatures, so
    // it's patched separately. The flag flips dry-run ↔ apply at runtime
    // without daemon restart — pipelines read `state.apply()` per request.
    let apply_change = body.get("apply").and_then(|v| v.as_bool());
    if let Some(new_apply) = apply_change {
        ds.set_apply(new_apply);
    }

    // Persist to global.toml so the change survives restart. Errors are
    // logged but not propagated — the in-memory state is already updated,
    // worst case the user has to re-toggle after a restart.
    let global_path = crate::config::GlobalConfig::default_path();
    if global_path.exists() {
        if let Ok(mut global) = crate::config::GlobalConfig::load(&global_path) {
            for entry in &mut global.repos {
                if entry.slug == slug {
                    entry.features = features.clone();
                    if let Some(new_apply) = apply_change {
                        entry.apply = Some(new_apply);
                    }
                }
            }
            if let Err(e) = global.save(&global_path) {
                tracing::warn!("Failed to persist features to global.toml: {e}");
            }
        }
    }

    let mut resp = json!(features);
    if let Some(obj) = resp.as_object_mut() {
        obj.insert("apply".to_string(), json!(ds.apply()));
    }
    Json(resp).into_response()
}

/// Enrich the stored domains with their live PR/issue volume and sort by it
/// descending, so the biggest subject-groupings (e.g. "codex" → 25) surface
/// first. Each entry gains a `count` field. Counts are computed live from the
/// title corpus, never persisted, so they stay fresh as PRs come and go.
fn domains_with_counts(
    db: &dyn crate::db::backend::DatabaseBackend,
    raw: serde_json::Value,
) -> serde_json::Value {
    let domains: Vec<crate::config::DomainDef> = serde_json::from_value(raw).unwrap_or_default();
    let counts = crate::pipelines::discover_domains::domain_counts(db, &domains);
    let mut arr: Vec<(usize, serde_json::Value)> = domains
        .iter()
        .map(|d| {
            let c = counts.get(&d.name).copied().unwrap_or(0);
            (
                c,
                json!({
                    "name": d.name,
                    "description": d.description,
                    "validated": d.validated,
                    "count": c,
                }),
            )
        })
        .collect();
    arr.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1["name"].as_str().cmp(&b.1["name"].as_str()))
    });
    json!(arr.into_iter().map(|(_, v)| v).collect::<Vec<_>>())
}

/// GET /api/v1/repos/{slug}/domains -- read the review "grand domains" list +
/// optional custom prompt. DB-backed (app_settings) so it survives on stateless
/// pods and is shared across replicas — unlike the ConfigMap-seeded global.toml.
async fn api_repo_domains_get(
    State(state): State<Arc<WebState>>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> Response {
    let repos = state.multi.repos.read().await;
    match repos.get(&slug) {
        Some(ds) => {
            let raw: serde_json::Value = ds
                .db
                .get_app_setting(crate::db::settings::REVIEW_DOMAINS_KEY)
                .ok()
                .flatten()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| json!([]));
            let domains = domains_with_counts(ds.db.as_ref(), raw);
            let prompt = ds
                .db
                .get_app_setting(crate::db::settings::REVIEW_PROMPT_KEY)
                .ok()
                .flatten();
            let limit = crate::pipelines::discover_domains::resolve_limit(ds.db.as_ref());
            Json(json!({ "domains": domains, "review_prompt": prompt, "limit": limit }))
                .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("repo '{slug}' not configured")})),
        )
            .into_response(),
    }
}

/// PATCH /api/v1/repos/{slug}/domains -- set the review domains + prompt. Body:
/// `{ "domains": [{ "name", "description" }], "review_prompt": "..." }` (either
/// field optional). Persists to the DB (app_settings), effective on the next
/// review pass across every pod.
async fn api_repo_domains_patch(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_admin(&user) {
            return e;
        }
    }

    let repos = state.multi.repos.read().await;
    let ds = match repos.get(&slug) {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("repo '{slug}' not configured")})),
            )
                .into_response();
        }
    };
    drop(repos);

    if let Some(d) = body.get("domains") {
        match serde_json::from_value::<Vec<crate::config::DomainDef>>(d.clone()) {
            Ok(parsed) => {
                let json_str = serde_json::to_string(&parsed).unwrap_or_else(|_| "[]".into());
                if let Err(e) = ds
                    .db
                    .set_app_setting(crate::db::settings::REVIEW_DOMAINS_KEY, &json_str)
                {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": e.to_string()})),
                    )
                        .into_response();
                }
            }
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("invalid domains: {e}")})),
                )
                    .into_response();
            }
        }
    }
    if let Some(p) = body.get("review_prompt") {
        let val = p.as_str().unwrap_or("");
        if let Err(e) = ds
            .db
            .set_app_setting(crate::db::settings::REVIEW_PROMPT_KEY, val)
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": e.to_string()})),
            )
                .into_response();
        }
    }
    // "Top N" discovery limit — clamped to the slider range before persisting.
    if let Some(l) = body.get("limit").and_then(|v| v.as_u64()) {
        let clamped = (l as usize).clamp(
            crate::pipelines::discover_domains::MIN_DOMAINS_LIMIT,
            crate::pipelines::discover_domains::MAX_DOMAINS_LIMIT,
        );
        let _ = ds.db.set_app_setting(
            crate::db::settings::REVIEW_DOMAINS_LIMIT_KEY,
            &clamped.to_string(),
        );
    }

    let raw: serde_json::Value = ds
        .db
        .get_app_setting(crate::db::settings::REVIEW_DOMAINS_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!([]));
    let domains = domains_with_counts(ds.db.as_ref(), raw);
    let prompt = ds
        .db
        .get_app_setting(crate::db::settings::REVIEW_PROMPT_KEY)
        .ok()
        .flatten();
    let limit = crate::pipelines::discover_domains::resolve_limit(ds.db.as_ref());
    Json(json!({ "domains": domains, "review_prompt": prompt, "limit": limit })).into_response()
}

/// POST /api/v1/repos/{slug}/domains/discover -- infer the repo's grand domains
/// from the frequency of subjects across ALL its PR & issue titles and merge
/// the top ones into the DB set as PROPOSED. Deterministic and instant (no AI),
/// so it never times out behind the auth proxy. Returns the full merged list.
async fn api_repo_domains_discover(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_admin(&user) {
            return e;
        }
    }

    let repos = state.multi.repos.read().await;
    let ds = match repos.get(&slug) {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("repo '{slug}' not configured")})),
            )
                .into_response();
        }
    };
    drop(repos);

    // Optional `?limit=N` slider value — persist it (clamped) so this and future
    // discovers use it.
    if let Some(n) = params.get("limit").and_then(|s| s.parse::<usize>().ok()) {
        let clamped = n.clamp(
            crate::pipelines::discover_domains::MIN_DOMAINS_LIMIT,
            crate::pipelines::discover_domains::MAX_DOMAINS_LIMIT,
        );
        let _ = ds.db.set_app_setting(
            crate::db::settings::REVIEW_DOMAINS_LIMIT_KEY,
            &clamped.to_string(),
        );
    }

    match crate::pipelines::discover_domains::discover(&ds.config, ds.db.as_ref()).await {
        Ok(domains) => {
            let enriched = domains_with_counts(
                ds.db.as_ref(),
                serde_json::to_value(&domains).unwrap_or_else(|_| json!([])),
            );
            let limit = crate::pipelines::discover_domains::resolve_limit(ds.db.as_ref());
            Json(json!({ "domains": enriched, "limit": limit })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("discovery failed: {e}")})),
        )
            .into_response(),
    }
}

/// GET /api/v1/config/retry -- read the live HTTP retry policy.
async fn api_retry_get() -> Response {
    Json(crate::retry::global()).into_response()
}

/// PATCH /api/v1/config/retry -- update the HTTP retry policy. Body is a
/// partial RetryConfig JSON (any subset of fields). Applies live without a
/// daemon restart and persists to ~/.wshm/global.toml.
async fn api_retry_patch(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_admin(&user) {
            return e;
        }
    }

    // Merge the partial body over the current live policy.
    let mut cfg = crate::retry::global();
    if let Some(v) = body.get("enabled").and_then(|v| v.as_bool()) {
        cfg.enabled = v;
    }
    if let Some(v) = body.get("max_attempts").and_then(|v| v.as_u64()) {
        cfg.max_attempts = v as u32;
    }
    if let Some(v) = body.get("initial_backoff_ms").and_then(|v| v.as_u64()) {
        cfg.initial_backoff_ms = v;
    }
    if let Some(v) = body.get("max_backoff_ms").and_then(|v| v.as_u64()) {
        cfg.max_backoff_ms = v;
    }
    // sanitized() clamps to safe ranges so a bad entry can't wedge retries.
    let cfg = cfg.sanitized();

    // Apply live, then persist so the policy survives a restart.
    crate::retry::set_global(cfg.clone());
    let global_path = crate::config::GlobalConfig::default_path();
    if global_path.exists() {
        if let Ok(mut global) = crate::config::GlobalConfig::load(&global_path) {
            global.retry = cfg.clone();
            if let Err(e) = global.save(&global_path) {
                tracing::warn!("Failed to persist retry config to global.toml: {e}");
            }
        }
    }

    Json(cfg).into_response()
}

/// POST /api/v1/repos -- add a repo at runtime (multi-repo mode only).
/// Body: {"slug": "owner/repo", "path": "/optional/abs/path"}
/// Requires `admin` role.
async fn api_add_repo(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_admin(&user) {
            return e;
        }
    }
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
    // A caller-supplied path is a directory-creation + SQLite-write primitive
    // (add_repo does create_dir_all + opens a DB there and loads any existing
    // config.toml), so reject absolute paths and any parent-dir traversal —
    // mirroring the hardening in api_restore_backup. add_repo then anchors the
    // value under the global config dir's repos/ subtree.
    let path_str = body
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(p) = path_str {
        let candidate = std::path::Path::new(p);
        if candidate.is_absolute() || candidate.components().any(|c| c.as_os_str() == "..") {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "path must be a relative name without '..' (it is anchored under the config repos/ directory)"
                })),
            )
                .into_response();
        }
    }
    let path = path_str.map(std::path::PathBuf::from);

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
    let prs = ds.db.get_open_pulls().unwrap_or_default();
    let conflicts = prs.iter().filter(|p| p.mergeable == Some(false)).count();

    // Untriaged count: derive it from the already-loaded open issues and the
    // triage-results key set instead of issuing a second full LEFT JOIN scan of
    // the issues table (the former get_untriaged_issues query). A single
    // get_all_triage_results() index read replaces that join.
    let triage = ds.db.get_all_triage_results().unwrap_or_default();
    let untriaged_count = issues
        .iter()
        .filter(|i| !triage.contains_key(&i.number))
        .count();

    // Batch-load all PR analyses once; the three passes below (unanalyzed
    // count, high-risk, top PRs) all index into this map instead of issuing
    // a per-PR get_pr_analysis query.
    let analyses = ds.db.get_all_pr_analyses().unwrap_or_default();

    // unanalyzed_prs: count PRs without an analysis row.
    let unanalyzed_count = prs
        .iter()
        .filter(|p| !analyses.contains_key(&p.number))
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

    // Top issues: by reactions+1, then by recency. Sort a Vec of
    // references to avoid a full deep clone of every issue just to take 5.
    let mut sorted_issues: Vec<&crate::db::issues::Issue> = issues.iter().collect();
    sorted_issues.sort_by(|a, b| {
        b.reactions_plus1
            .cmp(&a.reactions_plus1)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    let top_issues: Vec<_> = sorted_issues
        .iter()
        .take(5)
        .map(|i| to_issue_brief(i))
        .collect();

    // High-risk PRs: where analysis.risk_level is high or critical.
    let high_risk_prs: Vec<_> = prs
        .iter()
        .filter_map(|p| {
            analyses.get(&p.number).and_then(|a| {
                if a.risk_level == "high" || a.risk_level == "critical" {
                    Some(to_pr_brief(p, Some(a.risk_level.clone())))
                } else {
                    None
                }
            })
        })
        .take(5)
        .collect();

    // Top PRs: oldest open first (these are the ones at risk of being
    // forgotten). Sort references to avoid deep-cloning every PR.
    let mut sorted_prs: Vec<&crate::db::pulls::PullRequest> = prs.iter().collect();
    sorted_prs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let top_prs: Vec<_> = sorted_prs
        .iter()
        .take(5)
        .map(|p| {
            let risk = analyses.get(&p.number).map(|a| a.risk_level.clone());
            to_pr_brief(p, risk)
        })
        .collect();

    Json(json!({
        "repo": slug,
        "timestamp": now.to_rfc3339(),
        "open_issues": issues.len(),
        "untriaged_issues": untriaged_count,
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
///
/// Checks (in order): encrypted secret store (global scope), legacy
/// `.wshm/credentials` file, env vars. Returns `github=false` when none
/// is set so the UI can render an "Anonymous mode" banner.
async fn api_auth_status(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    let creds = crate::login::load_credentials();

    // Use the async get() (yields at the await point) rather than the
    // blocking variant, so this handler never holds a std Mutex across a
    // synchronous sqlite query on the tokio worker.
    let store = state.secrets.as_ref();
    let secrets_has = |key: &'static str| async move {
        if let Some(store) = store {
            if let Ok(Some(v)) = store.get(crate::secrets::Scope::Global, None, key).await {
                return !v.is_empty();
            }
        }
        false
    };

    let github = secrets_has("github_token").await
        || creds.contains_key("GITHUB_TOKEN")
        || std::env::var("GITHUB_TOKEN").is_ok()
        || std::env::var("WSHM_TOKEN").is_ok();

    let anthropic_kind = if secrets_has("anthropic_oauth_token").await
        || creds.contains_key("ANTHROPIC_OAUTH_TOKEN")
        || std::env::var("ANTHROPIC_OAUTH_TOKEN").is_ok()
        || std::env::var("CLAUDE_CODE_OAUTH_TOKEN").is_ok()
    {
        Some("oauth")
    } else if secrets_has("anthropic_api_key").await
        || creds.contains_key("ANTHROPIC_API_KEY")
        || std::env::var("ANTHROPIC_API_KEY").is_ok()
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

/// POST /api/v1/auth/github -- save the GitHub token. Prefers the
/// encrypted secret store (AES-256-GCM, master key from Vault on Pro
/// K8s); falls back to the plaintext credentials file when no store is
/// configured (OSS standalone). Hot-reloads every running GhClient.
async fn api_auth_github(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let admin = if state.users.is_some() {
        match require_admin(&user) {
            Ok(u) => Some(u),
            Err(e) => return e,
        }
    } else {
        None
    };

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

    // Storage strategy: secret store first (encrypted), file fallback.
    let backend_label = if let Some(store) = state.secrets.as_ref() {
        if let Err(e) = store
            .put(
                crate::secrets::Scope::Global,
                None,
                "github_token",
                &token,
                admin.as_ref().map(|u| u.id),
            )
            .await
        {
            tracing::warn!("Failed to persist GitHub token to secret store: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "error",
                    "message": "failed to persist GitHub token; see server logs",
                })),
            )
                .into_response();
        }
        // Once persisted in the encrypted store, scrub any legacy
        // plaintext copy so we don't keep two sources of truth.
        let mut creds = crate::login::load_credentials();
        if creds.remove("GITHUB_TOKEN").is_some() {
            if let Err(e) = crate::login::save_credentials(&creds) {
                // The token now lives in the encrypted store but the
                // legacy plaintext copy could NOT be scrubbed — surface it
                // so the operator knows the secret still exists on disk.
                tracing::warn!(
                    "Failed to scrub legacy plaintext GITHUB_TOKEN after encrypting: {e:#}"
                );
            }
        }
        "encrypted store"
    } else {
        // Fallback: legacy plaintext credentials file.
        let mut creds = crate::login::load_credentials();
        creds.insert("GITHUB_TOKEN".to_string(), token.clone());
        if let Err(e) = crate::login::save_credentials(&creds) {
            tracing::warn!("Failed to persist GitHub token to credentials file: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "error",
                    "message": "failed to persist GitHub token; see server logs",
                })),
            )
                .into_response();
        }
        "credentials file (plaintext)"
    };

    // Process env so subsequent Client::new() finds the token via the
    // env-var fallback if the secret store isn't queried.
    std::env::set_var("GITHUB_TOKEN", &token);

    // Hot-reload every running per-repo daemon's GhClient.
    reload_github_clients(&state, crate::secrets::Scope::Global, None).await;

    Json(json!({
        "status": "ok",
        "message": format!("GitHub token saved to {backend_label} and applied (no restart needed)"),
        "backend": backend_label,
    }))
    .into_response()
}

/// DELETE /api/v1/auth/github -- remove the configured GitHub token from
/// every storage backend (encrypted store + credentials file + process
/// env) and reload the GhClients so they revert to anonymous mode.
async fn api_auth_github_delete(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_admin(&user) {
            return e;
        }
    }

    // Remove from encrypted store.
    let mut removed_any = false;
    if let Some(store) = state.secrets.as_ref() {
        if let Ok(list) = store.list().await {
            for s in list {
                if s.scope == "global" && s.slug.is_none() && s.key == "github_token" {
                    match store.delete(s.id, None).await {
                        Ok(true) => removed_any = true,
                        Ok(false) => {}
                        Err(e) => tracing::warn!(
                            "Failed to delete github_token from encrypted store: {e:#}"
                        ),
                    }
                }
            }
        }
    }

    // Remove from plaintext file.
    let mut creds = crate::login::load_credentials();
    if creds.remove("GITHUB_TOKEN").is_some() {
        match crate::login::save_credentials(&creds) {
            Ok(()) => removed_any = true,
            Err(e) => tracing::warn!(
                "Failed to scrub GITHUB_TOKEN from plaintext credentials file: {e:#}"
            ),
        }
    }

    // Clear from process env.
    std::env::remove_var("GITHUB_TOKEN");

    // Reload GhClients so they go back to anonymous mode.
    reload_github_clients(&state, crate::secrets::Scope::Global, None).await;

    Json(json!({
        "status": "ok",
        "removed": removed_any,
        "message": "GitHub token cleared (anonymous mode active)",
    }))
    .into_response()
}

/// POST /api/v1/auth/anthropic -- store Anthropic OAuth token or API key.
/// Body: {"token": "...", "kind": "oauth"|"api_key"}
/// Same encrypted-store-first / file-fallback strategy as api_auth_github.
async fn api_auth_anthropic(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let admin = if state.users.is_some() {
        match require_admin(&user) {
            Ok(u) => Some(u),
            Err(e) => return e,
        }
    } else {
        None
    };

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
    let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("oauth");

    let (env_key, secret_key, other_env, other_secret) = match kind {
        "oauth" => (
            "ANTHROPIC_OAUTH_TOKEN",
            "anthropic_oauth_token",
            "ANTHROPIC_API_KEY",
            "anthropic_api_key",
        ),
        "api_key" => (
            "ANTHROPIC_API_KEY",
            "anthropic_api_key",
            "ANTHROPIC_OAUTH_TOKEN",
            "anthropic_oauth_token",
        ),
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "kind must be 'oauth' or 'api_key'"})),
            )
                .into_response();
        }
    };

    let backend_label = if let Some(store) = state.secrets.as_ref() {
        // Drop the mutually-exclusive other key from the store so
        // resolve_anthropic_auth's priority isn't confused.
        if let Ok(list) = store.list().await {
            for s in list {
                if s.scope == "global" && s.slug.is_none() && s.key == other_secret {
                    if let Err(e) = store.delete(s.id, None).await {
                        tracing::warn!(
                            "Failed to delete stale {other_secret} from encrypted store: {e:#}"
                        );
                    }
                }
            }
        }
        if let Err(e) = store
            .put(
                crate::secrets::Scope::Global,
                None,
                secret_key,
                &token,
                admin.as_ref().map(|u| u.id),
            )
            .await
        {
            tracing::warn!("Failed to persist Anthropic token to secret store: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "error",
                    "message": "failed to persist token; see server logs",
                })),
            )
                .into_response();
        }
        // Scrub any plaintext copy.
        let mut creds = crate::login::load_credentials();
        let mut scrubbed = false;
        scrubbed |= creds.remove(env_key).is_some();
        scrubbed |= creds.remove(other_env).is_some();
        if scrubbed {
            if let Err(e) = crate::login::save_credentials(&creds) {
                // Token is in the encrypted store but the legacy plaintext
                // copy could not be scrubbed — warn so the operator knows.
                tracing::warn!(
                    "Failed to scrub legacy plaintext Anthropic token after encrypting: {e:#}"
                );
            }
        }
        "encrypted store"
    } else {
        let mut creds = crate::login::load_credentials();
        creds.remove(other_env);
        creds.insert(env_key.to_string(), token.clone());
        if let Err(e) = crate::login::save_credentials(&creds) {
            tracing::warn!("Failed to persist Anthropic token to credentials file: {e:#}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "status": "error",
                    "message": "failed to persist token; see server logs",
                })),
            )
                .into_response();
        }
        "credentials file (plaintext)"
    };

    std::env::set_var(env_key, &token);
    std::env::remove_var(other_env);

    Json(json!({
        "status": "ok",
        "kind": kind,
        "backend": backend_label,
        "message": format!("{env_key} saved to {backend_label}"),
    }))
    .into_response()
}

/// DELETE /api/v1/auth/anthropic -- remove both Anthropic credentials
/// (OAuth token + API key) from every backend.
async fn api_auth_anthropic_delete(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
) -> Response {
    if state.users.is_some() {
        if let Err(e) = require_admin(&user) {
            return e;
        }
    }

    let mut removed_any = false;
    if let Some(store) = state.secrets.as_ref() {
        if let Ok(list) = store.list().await {
            for s in list {
                if s.scope == "global"
                    && s.slug.is_none()
                    && (s.key == "anthropic_oauth_token" || s.key == "anthropic_api_key")
                {
                    match store.delete(s.id, None).await {
                        Ok(true) => removed_any = true,
                        Ok(false) => {}
                        Err(e) => {
                            tracing::warn!("Failed to delete {} from encrypted store: {e:#}", s.key)
                        }
                    }
                }
            }
        }
    }

    let mut creds = crate::login::load_credentials();
    let before = creds.len();
    creds.remove("ANTHROPIC_OAUTH_TOKEN");
    creds.remove("ANTHROPIC_API_KEY");
    if creds.len() != before {
        match crate::login::save_credentials(&creds) {
            Ok(()) => removed_any = true,
            Err(e) => tracing::warn!(
                "Failed to scrub Anthropic tokens from plaintext credentials file: {e:#}"
            ),
        }
    }

    std::env::remove_var("ANTHROPIC_OAUTH_TOKEN");
    std::env::remove_var("ANTHROPIC_API_KEY");

    Json(json!({
        "status": "ok",
        "removed": removed_any,
        "message": "Anthropic credentials cleared",
    }))
    .into_response()
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Encrypted secrets API (admin only)
// ---------------------------------------------------------------------------

/// GET /api/v1/secrets -- list all stored secrets, values masked.
async fn api_secrets_list(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
) -> Response {
    if let Err(e) = require_admin(&user) {
        return e;
    }
    let store = match state.secrets.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "secret store not configured (set WSHM_MASTER_KEY)"})),
            )
                .into_response();
        }
    };
    match store.list().await {
        Ok(rows) => Json(json!({ "secrets": rows })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// POST /api/v1/secrets -- upsert a secret. Body:
///   {"scope":"global"|"repo", "slug":"owner/repo"?, "key":"...", "value":"..."}
async fn api_secrets_put(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    tracing::debug!(
        target: "wshm_core::secrets_trace",
        "api_secrets_put: ENTERED, body keys = {:?}",
        body.as_object().map(|o| o.keys().collect::<Vec<_>>())
    );
    let admin = match require_admin(&user) {
        Ok(u) => {
            tracing::debug!(
                target: "wshm_core::secrets_trace",
                "api_secrets_put: admin check PASSED, user_id={}",
                u.id
            );
            u
        }
        Err(e) => {
            tracing::warn!(
                target: "wshm_core::secrets_trace",
                "api_secrets_put: admin check FAILED — request rejected"
            );
            return e;
        }
    };
    let store = match state.secrets.as_ref() {
        Some(s) => {
            tracing::debug!(
                target: "wshm_core::secrets_trace",
                "api_secrets_put: secret store available"
            );
            s
        }
        None => {
            tracing::warn!(
                target: "wshm_core::secrets_trace",
                "api_secrets_put: NO secret store configured — returning 503"
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "secret store not configured"})),
            )
                .into_response();
        }
    };
    let scope_str = body.get("scope").and_then(|v| v.as_str()).unwrap_or("");
    let scope = match crate::secrets::Scope::from_str(scope_str) {
        Ok(s) => s,
        Err(_) => {
            tracing::warn!(
                target: "wshm_core::secrets_trace",
                "api_secrets_put: invalid scope={scope_str:?} — returning 400"
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "scope must be 'global' or 'repo'"})),
            )
                .into_response();
        }
    };
    let slug = body.get("slug").and_then(|v| v.as_str());
    let key = body.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let value = body.get("value").and_then(|v| v.as_str()).unwrap_or("");
    tracing::debug!(
        target: "wshm_core::secrets_trace",
        "api_secrets_put: parsed scope={:?} slug={:?} key={:?} value_len={}",
        scope, slug, key, value.len()
    );
    if key.trim().is_empty() || value.is_empty() {
        tracing::warn!(
            target: "wshm_core::secrets_trace",
            "api_secrets_put: validation FAILED — key or value empty"
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "key and value are required"})),
        )
            .into_response();
    }
    if scope == crate::secrets::Scope::Repo && slug.map(str::trim).is_none_or(|s| s.is_empty()) {
        tracing::warn!(
            target: "wshm_core::secrets_trace",
            "api_secrets_put: validation FAILED — Repo scope requires non-empty slug"
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "slug is required for scope=repo"})),
        )
            .into_response();
    }
    let effective_slug = if scope == crate::secrets::Scope::Repo {
        slug
    } else {
        None
    };
    tracing::debug!(
        target: "wshm_core::secrets_trace",
        "api_secrets_put: calling store.put(scope={:?}, slug={:?}, key={:?})",
        scope, effective_slug, key.trim()
    );
    match store
        .put(scope, effective_slug, key.trim(), value, Some(admin.id))
        .await
    {
        Ok(id) => {
            tracing::debug!(
                target: "wshm_core::secrets_trace",
                "api_secrets_put: store.put OK — row id={id}"
            );
            // Hot-reload affected daemon clients so the new token / API key
            // takes effect without a restart. Only github_token reloads the
            // GhClient today; other keys are read on-demand by the relevant
            // pipeline so no reload is needed.
            if key.trim() == "github_token" {
                tracing::debug!(
                    target: "wshm_core::secrets_trace",
                    "api_secrets_put: key is github_token — triggering reload"
                );
                reload_github_clients(&state, scope, effective_slug).await;
            } else {
                tracing::debug!(
                    target: "wshm_core::secrets_trace",
                    "api_secrets_put: key={:?} ≠ github_token — no reload",
                    key.trim()
                );
            }
            (StatusCode::CREATED, Json(json!({ "id": id }))).into_response()
        }
        Err(e) => {
            tracing::error!(
                target: "wshm_core::secrets_trace",
                "api_secrets_put: store.put FAILED — {e:#}"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("{e}")})),
            )
                .into_response()
        }
    }
}

/// Rebuild the GitHub client of every running per-repo daemon whose scope
/// matches the secret that was just written or removed. Errors are logged
/// but never propagate — a failed reload doesn't mean the secret write
/// itself failed.
async fn reload_github_clients(state: &WebState, scope: crate::secrets::Scope, slug: Option<&str>) {
    let repos_guard = state.multi.repos.read().await;
    tracing::debug!(
        target: "wshm_core::secrets_trace",
        "reload_github_clients: scope={:?} slug={:?}, iterating {} repos",
        scope, slug, repos_guard.len()
    );
    for (repo_slug, ds) in repos_guard.iter() {
        // global secret affects every repo; repo-scoped only affects its owner.
        let matches = match scope {
            crate::secrets::Scope::Global => true,
            crate::secrets::Scope::Repo => slug == Some(repo_slug.as_str()),
        };
        if !matches {
            tracing::debug!(
                target: "wshm_core::secrets_trace",
                "reload_github_clients: SKIPPING [{repo_slug}] (slug mismatch)"
            );
            continue;
        }
        tracing::debug!(
            target: "wshm_core::secrets_trace",
            "reload_github_clients: RELOADING [{repo_slug}]"
        );
        if let Err(e) = ds.reload_github_client() {
            tracing::warn!("[{repo_slug}] reload_github_client failed: {e:#}");
        }
    }
}

/// POST /api/v1/secrets/{id}/reveal -- decrypt and return the plaintext.
async fn api_secrets_reveal(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    let admin = match require_admin(&user) {
        Ok(u) => u,
        Err(e) => return e,
    };
    let store = match state.secrets.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "secret store not configured"})),
            )
                .into_response();
        }
    };
    match store.reveal(id, Some(admin.id)).await {
        Ok(Some(v)) => Json(json!({ "value": v })).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// DELETE /api/v1/secrets/{id}
async fn api_secrets_delete(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Response {
    let admin = match require_admin(&user) {
        Ok(u) => u,
        Err(e) => return e,
    };
    let store = match state.secrets.as_ref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "secret store not configured"})),
            )
                .into_response();
        }
    };
    match store.delete(id, Some(admin.id)).await {
        Ok(true) => {
            // Blanket-reload — we don't know if the deleted secret was a
            // github_token without looking it up first. Rebuilding a few
            // GhClients is cheap; deletes are rare admin actions.
            reload_github_clients(&state, crate::secrets::Scope::Global, None).await;
            Json(json!({"status": "ok"})).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("{e}")})),
        )
            .into_response(),
    }
}

/// GET /api/v1/logs -- tail of the daemon's in-memory log buffer.
///
/// Query: `tail` (default 200, max 5000), `level` (ERROR/WARN/INFO/DEBUG/TRACE),
/// `since` (numeric id — return only entries newer than this id).
///
/// Requires Operator role (or higher) when RBAC is enabled — log entries
/// can include diagnostic context that members/viewers should not see.
async fn api_logs(
    State(state): State<Arc<WebState>>,
    user: axum::Extension<Option<crate::auth::User>>,
    Query(params): Query<LogsQuery>,
) -> impl IntoResponse {
    if state.users.is_some() {
        if let Err(e) = require_min_role(&user, crate::auth::Role::Operator) {
            return e;
        }
    }
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
        .route("/api/v1/sync/incremental", post(api_sync_incremental))
        .route("/api/v1/sync/full", post(api_sync_full))
        .route("/api/v1/license", get(api_license))
        .route("/api/v1/license/activate", post(api_license_activate))
        .route("/api/v1/repos", get(api_list_repos).post(api_add_repo))
        .route(
            "/api/v1/config/retry",
            get(api_retry_get).patch(api_retry_patch),
        )
        .route(
            "/api/v1/repos/{slug}/features",
            get(api_repo_features_get).patch(api_repo_features_patch),
        )
        .route(
            "/api/v1/repos/{slug}/domains",
            get(api_repo_domains_get).patch(api_repo_domains_patch),
        )
        .route(
            "/api/v1/repos/{slug}/domains/discover",
            post(api_repo_domains_discover),
        )
        .route("/api/v1/auth/status", get(api_auth_status))
        .route(
            "/api/v1/auth/github",
            post(api_auth_github).delete(api_auth_github_delete),
        )
        .route(
            "/api/v1/auth/anthropic",
            post(api_auth_anthropic).delete(api_auth_anthropic_delete),
        )
        .route("/api/v1/auth/login", post(api_auth_login))
        .route("/api/v1/auth/logout", post(api_auth_logout))
        .route("/api/v1/auth/me", get(api_auth_me))
        .route("/api/v1/users", get(api_users_list).post(api_users_create))
        .route(
            "/api/v1/users/{id}",
            axum::routing::patch(api_users_update).delete(api_users_delete),
        )
        .route("/api/v1/logs", get(api_logs))
        .route(
            "/api/v1/secrets",
            get(api_secrets_list).post(api_secrets_put),
        )
        .route(
            "/api/v1/secrets/{id}",
            axum::routing::delete(api_secrets_delete),
        )
        .route("/api/v1/secrets/{id}/reveal", post(api_secrets_reveal))
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
    web_routes_with_extensions(multi, None, None, None, None, None)
}

/// Build the web UI router with optional extensions.
///
/// - `users`: enable RBAC mode by passing a populated UserStore. `None`
///   keeps the legacy single-credential `[web].username/password` flow.
///
///   SECURITY: in legacy mode (`users = None`) there is no role model, so the
///   privileged, secret-writing token-management endpoints
///   (`POST`/`DELETE /api/v1/auth/github` and `/api/v1/auth/anthropic`) cannot
///   enforce an admin gate — any authenticated caller (the single shared web
///   password / trusted proxy identity) can write or delete the GitHub and
///   Anthropic tokens. The OSS `run()` path always opens a UserStore so RBAC is
///   active there; embedders MUST NOT use `users = None` with a
///   network-exposed listener.
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
    users: Option<Arc<dyn crate::auth::UserStoreBackend>>,
    logs: Option<Arc<crate::daemon::log_buffer::LogBuffer>>,
    secrets: Option<Arc<dyn crate::secrets::SecretStore>>,
    extra_api: Option<Router<Arc<WebState>>>,
    spa_override: Option<Router<Arc<WebState>>>,
) -> Router {
    let state = Arc::new(WebState {
        multi,
        users,
        logs,
        secrets,
    });

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
