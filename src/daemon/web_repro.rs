//! REST handlers for automatic issue reproduction (#229). Child module of
//! `daemon::web` so it can reuse the private auth helpers.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::sync::Arc;

use super::{require_admin, WebState};
use crate::pipelines::repro::{ReproConfig, REPRO_SETTINGS_KEY};

fn not_found(slug: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": format!("repo '{slug}' not configured")})),
    )
        .into_response()
}

/// GET /api/v1/repos/{slug}/repro -- effective `[triage.repro]` settings
/// (TOML defaults + per-repo DB override), the raw override and whether the
/// feature can run at all here.
pub(super) async fn api_repo_repro_get(
    State(state): State<Arc<WebState>>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> Response {
    let Some(ds) = state.multi.repos.read().await.get(&slug).cloned() else {
        return not_found(&slug);
    };
    let effective = ReproConfig::effective(&ds.config.triage.repro, ds.db.as_ref());
    let raw = ds
        .db
        .get_app_setting(REPRO_SETTINGS_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    Json(json!({
        "enabled": ds.features().repro_issues,
        "sandbox_available": crate::pro_hooks::repro_sandbox().is_some(),
        "settings": effective,
        "override": raw,
    }))
    .into_response()
}

/// PATCH /api/v1/repos/{slug}/repro -- replace the per-repo override with
/// the JSON object in the body (any subset of fields; `{}` resets to the
/// TOML defaults). Stored in the DB so every pod sees it.
pub(super) async fn api_repo_repro_patch(
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
    let Some(ds) = state.multi.repos.read().await.get(&slug).cloned() else {
        return not_found(&slug);
    };
    let raw = body.to_string();
    let effective = match ds.config.triage.repro.overlay(&raw) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("{e:#}")})),
            )
                .into_response();
        }
    };
    if let Err(e) = ds.db.set_app_setting(REPRO_SETTINGS_KEY, &raw) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response();
    }
    Json(json!({ "settings": effective, "override": body })).into_response()
}

/// GET /api/v1/repos/{slug}/issues/{number}/repro -- latest reproduction
/// result of an issue (`run: null` when never reproduced).
pub(super) async fn api_issue_repro_get(
    State(state): State<Arc<WebState>>,
    axum::extract::Path((slug, number)): axum::extract::Path<(String, u64)>,
) -> Response {
    let Some(ds) = state.multi.repos.read().await.get(&slug).cloned() else {
        return not_found(&slug);
    };
    match ds.db.latest_repro_run(number) {
        Ok(run) => Json(json!({
            "enabled": ds.features().repro_issues,
            "run": run,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
