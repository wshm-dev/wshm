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
    routing::{get, post},
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

/// Shared state for all web UI routes (OSS and Pro).
///
/// Exposed so that extension crates (e.g. `wshm-pro`) can build additional
/// `Router<Arc<WebState>>` routers and merge them into the main router via
/// [`web_routes_with_extensions`].
pub struct WebState {
    pub multi: Arc<MultiDaemonState>,
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

    for (slug, ds) in &state.multi.repos {
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

    for (slug, ds) in &state.multi.repos {
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

    for (slug, ds) in &state.multi.repos {
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

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

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
    web_routes_with_extensions(multi, None, None)
}

/// Build the web UI router with optional extensions.
///
/// - `extra_api`: additional `Router<Arc<WebState>>` whose routes get merged
///   into the main router under the same auth layer. Use this to register
///   Pro API endpoints from extension crates.
/// - `spa_override`: replaces the default embedded Svelte SPA router. Use
///   this to serve a different web-dist bundle (e.g. wshm-pro's full
///   OSS+Pro build).
///
/// The `WebState` is built from `multi` and shared with every sub-router.
pub fn web_routes_with_extensions(
    multi: Arc<MultiDaemonState>,
    extra_api: Option<Router<Arc<WebState>>>,
    spa_override: Option<Router<Arc<WebState>>>,
) -> Router {
    let state = Arc::new(WebState { multi });

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
