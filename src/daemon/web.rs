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
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::Engine;
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use serde_json::json;
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

struct WebState {
    multi: Arc<MultiDaemonState>,
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

/// Basic-auth middleware layer.  Allows `/health` without credentials.
/// Checks the `Authorization: Basic <base64(user:pass)>` header against
/// the values in the first repo's `[web]` config (username / password).
/// If no password is configured, auth is disabled (all requests pass).
async fn auth_middleware(
    State(state): State<Arc<WebState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // /health is always public
    if req.uri().path() == "/health" {
        return next.run(req).await;
    }

    // Grab web config from the first available repo (all repos share the
    // same daemon process, so one set of web credentials is sufficient).
    let web_cfg = match state.multi.repos.values().next() {
        Some(ds) => &ds.config.web,
        None => return next.run(req).await,
    };

    // If no password is set, auth is disabled.
    let required_password = match &web_cfg.password {
        Some(p) => p,
        None => return next.run(req).await,
    };

    let expected_username = &web_cfg.username;

    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|b64| {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .ok()
        })
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|decoded| {
            if let Some((user, pass)) = decoded.split_once(':') {
                user == expected_username && pass == required_password
            } else {
                false
            }
        })
        .unwrap_or(false);

    if authorized {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"wshm\"")],
            Json(json!({"error": "unauthorized"})),
        )
            .into_response()
    }
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

    for (slug, ds) in &state.multi.repos {
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
            if resp.last_sync.as_ref().map_or(true, |cur| ls > cur) {
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

    for (slug, ds) in &state.multi.repos {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }
        if let Ok(issues) = ds.db.get_open_issues() {
            for issue in issues {
                let triage = ds.db.get_triage_result(issue.number).ok().flatten();
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

    for (slug, ds) in &state.multi.repos {
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

    for (slug, ds) in &state.multi.repos {
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

    for (slug, ds) in &state.multi.repos {
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

    for (slug, ds) in &state.multi.repos {
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
        None => (StatusCode::NOT_FOUND, "index.html not found in embedded assets").into_response(),
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

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

/// Build the web UI router.  Merge this into the existing axum server or
/// use it standalone.
///
/// All `/api/v1/*` routes require basic auth (when a password is configured).
/// The `/health` endpoint is always public.
/// All other routes serve the embedded Svelte SPA.
pub fn web_routes(multi: Arc<MultiDaemonState>) -> Router {
    let state = Arc::new(WebState { multi });

    let api_routes = Router::new()
        .route("/api/v1/status", get(api_status))
        .route("/api/v1/issues", get(api_issues))
        .route("/api/v1/pulls", get(api_pulls))
        .route("/api/v1/triage", get(api_triage))
        .route("/api/v1/queue", get(api_queue))
        .route("/api/v1/activity", get(api_activity));

    let spa_routes = Router::new()
        .route("/", get(handle_spa_root))
        .fallback(handle_spa_fallback);

    Router::new()
        .merge(api_routes)
        .merge(spa_routes)
        .layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            auth_middleware,
        ))
        .with_state(state)
}
