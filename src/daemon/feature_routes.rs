//! Daemon API routes for review, conflicts, improvements, reports, auto-fix
//! candidates, search, PR/issue insights and usage.
//!
//! Merged into [`super::web::oss_api_routes`], so they sit behind the same
//! auth layer as every other `/api/v1/*` route.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::daemon::web::WebState;

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RepoFilter {
    repo: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: Option<String>,
    repo: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

const SEARCH_DEFAULT_LIMIT: usize = 50;
const SEARCH_MAX_LIMIT: usize = 250;
const SEARCH_PER_REPO_CAP: usize = 200;

// ---------------------------------------------------------------------------
// API handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/reviews -- PRs with review/analysis data.
async fn api_reviews(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut reviews = Vec::new();

    let repos_guard = state.multi.repos.read().await;
    for (slug, ds) in repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }
        let Ok(prs) = ds.db.get_open_pulls() else {
            continue;
        };
        let Ok(all_reviews) = ds.db.get_all_pr_reviews() else {
            continue;
        };
        for pr in prs {
            if let Some(r) = all_reviews.get(&pr.number) {
                reviews.push(json!({
                    "repo": slug,
                    "pr_number": pr.number,
                    "title": pr.title,
                    "author": pr.author,
                    "head_ref": pr.head_ref,
                    "base_ref": pr.base_ref,
                    "summary": r.result.summary,
                    "stats": r.result.stats,
                    "comments": r.result.comments,
                    "posted_to_github": r.posted_to_github,
                    "reviewed_at": r.reviewed_at,
                    "ci_status": pr.ci_status,
                    "mergeable": pr.mergeable,
                }));
            }
        }
    }

    reviews.sort_by(|a, b| {
        let da = a["reviewed_at"].as_str().unwrap_or("");
        let db = b["reviewed_at"].as_str().unwrap_or("");
        db.cmp(da)
    });

    Json(reviews)
}

/// GET /api/v1/conflicts -- conflict scan across open PRs.
async fn api_conflicts(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut result = json!({
        "total": 0,
        "clean": 0,
        "conflicts": [],
        "unknown": 0,
    });

    let mut conflicts_list: Vec<serde_json::Value> = Vec::new();
    let mut total = 0usize;
    let mut clean = 0usize;
    let mut unknown = 0usize;

    let repos_guard = state.multi.repos.read().await;
    for (slug, ds) in repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }
        if let Ok(prs) = ds.db.get_open_pulls() {
            for pr in prs {
                total += 1;
                match pr.mergeable {
                    Some(true) => clean += 1,
                    Some(false) => {
                        conflicts_list.push(json!({
                            "repo": slug,
                            "number": pr.number,
                            "title": pr.title,
                            "author": pr.author,
                            "base_ref": pr.base_ref,
                            "head_ref": pr.head_ref,
                            "ci_status": pr.ci_status,
                            "created_at": pr.created_at,
                            "updated_at": pr.updated_at,
                        }));
                    }
                    None => unknown += 1,
                }
            }
        }
    }

    result["total"] = json!(total);
    result["clean"] = json!(clean);
    result["conflicts"] = json!(conflicts_list);
    result["unknown"] = json!(unknown);

    Json(result)
}

/// GET /api/v1/improvements -- improvement suggestions.
async fn api_improvements(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut suggestions = Vec::new();

    let repos_guard = state.multi.repos.read().await;
    for (slug, ds) in repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }

        if let Ok(issues) = ds.db.get_open_issues() {
            for issue in &issues {
                let is_improvement = issue.labels.iter().any(|l| {
                    l.contains("improvement")
                        || l.contains("refactor")
                        || l.contains("wshm-improve")
                });
                if is_improvement {
                    let triage = ds.db.get_triage_result(issue.number).ok().flatten();
                    let category = issue
                        .labels
                        .iter()
                        .find(|l| {
                            [
                                "refactor",
                                "performance",
                                "testing",
                                "security",
                                "feature",
                                "docs",
                            ]
                            .iter()
                            .any(|c| l.contains(c))
                        })
                        .cloned()
                        .unwrap_or_else(|| "refactor".to_string());

                    let effort = issue
                        .labels
                        .iter()
                        .find(|l| l.starts_with("effort:"))
                        .map(|l| l.trim_start_matches("effort:").to_string())
                        .unwrap_or_else(|| "medium".to_string());

                    suggestions.push(json!({
                        "repo": slug,
                        "title": issue.title,
                        "body": issue.body,
                        "category": category,
                        "effort": effort,
                        "issue_number": issue.number,
                        "priority": triage.as_ref().and_then(|t| t.priority.as_deref()),
                        "created_at": issue.created_at,
                    }));
                }
            }
        }
    }

    Json(json!({ "suggestions": suggestions }))
}

/// GET /api/v1/reports -- repo health report data.
async fn api_reports(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut report = json!({
        "repos": [],
    });

    let mut repos_data = Vec::new();

    let repos_guard = state.multi.repos.read().await;
    for (slug, ds) in repos_guard.iter() {
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

        let now = chrono::Utc::now();
        let pr_ages: Vec<f64> = open_prs
            .iter()
            .filter_map(|pr| {
                chrono::DateTime::parse_from_rfc3339(&pr.created_at)
                    .ok()
                    .map(|d| (now - d.with_timezone(&chrono::Utc)).num_hours() as f64 / 24.0)
            })
            .collect();
        let issue_ages: Vec<f64> = open_issues
            .iter()
            .filter_map(|i| {
                chrono::DateTime::parse_from_rfc3339(&i.created_at)
                    .ok()
                    .map(|d| (now - d.with_timezone(&chrono::Utc)).num_hours() as f64 / 24.0)
            })
            .collect();

        let avg_pr_age = if pr_ages.is_empty() {
            0.0
        } else {
            pr_ages.iter().sum::<f64>() / pr_ages.len() as f64
        };
        let avg_issue_age = if issue_ages.is_empty() {
            0.0
        } else {
            issue_ages.iter().sum::<f64>() / issue_ages.len() as f64
        };
        let prs_over_7d = pr_ages.iter().filter(|a| **a > 7.0).count();
        let prs_over_30d = pr_ages.iter().filter(|a| **a > 30.0).count();
        let issues_over_7d = issue_ages.iter().filter(|a| **a > 7.0).count();
        let issues_over_30d = issue_ages.iter().filter(|a| **a > 30.0).count();

        let mut categories: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        if let Ok(results) = ds.db.recent_activity(200) {
            for r in &results {
                *categories.entry(r.category.clone()).or_default() += 1;
            }
        }

        let mut risks: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for pr in &open_prs {
            if let Ok(Some(a)) = ds.db.get_pr_analysis(pr.number) {
                *risks.entry(a.risk_level).or_default() += 1;
            }
        }

        repos_data.push(json!({
            "slug": slug,
            "open_issues": open_issues.len(),
            "untriaged": untriaged.len(),
            "open_prs": open_prs.len(),
            "unanalyzed": unanalyzed.len(),
            "conflicts": conflicts,
            "sla": {
                "avg_pr_age_days": (avg_pr_age * 10.0).round() / 10.0,
                "avg_issue_age_days": (avg_issue_age * 10.0).round() / 10.0,
                "prs_over_7d": prs_over_7d,
                "prs_over_30d": prs_over_30d,
                "issues_over_7d": issues_over_7d,
                "issues_over_30d": issues_over_30d,
            },
            "categories": categories,
            "risks": risks,
        }));
    }

    report["repos"] = json!(repos_data);
    Json(report)
}

/// GET /api/v1/autofix/candidates -- issues eligible for auto-fix.
async fn api_autofix_candidates(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut candidates = Vec::new();

    let repos_guard = state.multi.repos.read().await;
    for (slug, ds) in repos_guard.iter() {
        if let Some(ref f) = filter.repo {
            if f != slug {
                continue;
            }
        }

        if let Ok(issues) = ds.db.get_open_issues() {
            for issue in &issues {
                if let Ok(Some(triage)) = ds.db.get_triage_result(issue.number) {
                    if triage.is_simple_fix && triage.confidence >= 0.85 {
                        candidates.push(json!({
                            "repo": slug,
                            "number": issue.number,
                            "title": issue.title,
                            "category": triage.category,
                            "confidence": triage.confidence,
                            "priority": triage.priority,
                            "summary": triage.summary,
                            "created_at": issue.created_at,
                        }));
                    }
                }
            }
        }
    }

    candidates.sort_by(|a, b| {
        let ca = a["confidence"].as_f64().unwrap_or(0.0);
        let cb = b["confidence"].as_f64().unwrap_or(0.0);
        cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
    });

    Json(json!({ "candidates": candidates }))
}

/// GET /api/v1/search -- cross-entity full-text search.
///
/// Query string:
///   q      - free-form search terms (required, sanitised before MATCH)
///   repo   - optional repo slug to scope a single repo
///   limit  - default 50, clamped to [1, 250]
///   offset - default 0
///
/// Response: `{items, total, limit, offset}` where each item is
/// `{kind, repo, number, title, snippet, updated_at}`. Items are ranked
/// by FTS5 BM25 across all matching repos.
async fn api_search(
    State(state): State<Arc<WebState>>,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    let raw = q.q.as_deref().unwrap_or("").trim();
    if raw.is_empty() {
        return (
            StatusCode::OK,
            Json(json!({
                "items": [],
                "total": 0,
                "limit": q.limit.unwrap_or(SEARCH_DEFAULT_LIMIT),
                "offset": q.offset.unwrap_or(0),
            })),
        )
            .into_response();
    }

    let limit = q
        .limit
        .unwrap_or(SEARCH_DEFAULT_LIMIT)
        .clamp(1, SEARCH_MAX_LIMIT);
    let offset = q.offset.unwrap_or(0);

    let mut hits: Vec<serde_json::Value> = Vec::new();
    let repos = state.multi.repos.read().await;
    for (slug, ds) in repos.iter() {
        if let Some(ref f) = q.repo {
            if f != slug {
                continue;
            }
        }
        // The trait dispatches sanitisation per backend (FTS5 prefix-AND on
        // SQLite, websearch_to_tsquery on Postgres), so we pass the raw
        // user input straight through.
        match ds.db.search_fts(raw, SEARCH_PER_REPO_CAP) {
            Ok(rows) => {
                for h in rows {
                    hits.push(json!({
                        "kind": h.kind,
                        "repo": slug,
                        "number": h.number,
                        "title": h.title,
                        "snippet": h.snippet,
                        "updated_at": h.updated_at,
                        "rank": h.rank,
                    }));
                }
            }
            Err(e) => {
                tracing::warn!("[{slug}] search_fts failed: {e:#}");
            }
        }
    }
    drop(repos);

    // Cross-repo merge: BM25 lower = better.
    hits.sort_by(|a, b| {
        let ra = a["rank"].as_f64().unwrap_or(f64::INFINITY);
        let rb = b["rank"].as_f64().unwrap_or(f64::INFINITY);
        ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
    });

    let total = hits.len();
    let end = (offset + limit).min(total);
    let page: Vec<serde_json::Value> = if offset >= total {
        Vec::new()
    } else {
        hits.into_iter().skip(offset).take(end - offset).collect()
    };

    (
        StatusCode::OK,
        Json(json!({
            "items": page,
            "total": total,
            "limit": limit,
            "offset": offset,
        })),
    )
        .into_response()
}

/// GET /api/v1/pr-insights -- aggregate dashboard for open PRs.
///
/// Returns every open PR across configured repos, each tagged with a
/// `status` (`mergeable` / `conflict` / `unknown`, derived from the
/// `mergeable` flag), plus `default_branches`: the set of per-repo
/// `[fix].base_branch` values. The SPA derives counts and charts from
/// this; surfacing the default branches lets the UI highlight them in
/// the target-branch breakdown rather than hard-coding `master`/`develop`.
async fn api_pr_insights(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut items = Vec::new();
    let mut default_branches: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    let repos_guard = state.multi.repos.read().await;
    for (slug, ds) in repos_guard.iter() {
        let matches = filter.repo.as_ref().is_none_or(|f| f == slug);
        if matches {
            default_branches.insert(ds.config.fix.base_branch.clone());
        }
        let Ok(prs) = ds.db.get_open_pulls() else {
            continue;
        };

        // (Trend history is reconstructed retroactively by api_pr_trend from
        // created_at/updated_at, so no daily snapshot is recorded here.)

        if !matches {
            continue;
        }
        for pr in prs {
            let analysis = ds.db.get_pr_analysis(pr.number).ok().flatten();
            let status = match pr.mergeable {
                Some(true) => "mergeable",
                Some(false) => "conflict",
                None => "unknown",
            };
            items.push(json!({
                "repo": slug,
                "number": pr.number,
                "title": pr.title,
                "author": pr.author,
                "head_ref": pr.head_ref,
                "base_ref": pr.base_ref,
                "mergeable": pr.mergeable,
                "status": status,
                "ci_status": pr.ci_status,
                "risk_level": analysis.as_ref().map(|a| a.risk_level.as_str()),
                "summary": analysis.as_ref().map(|a| a.summary.as_str()),
                "created_at": pr.created_at,
                "updated_at": pr.updated_at,
                "url": crate::git_provider::web_url_for_pr(&ds.config, pr.number),
            }));
        }
    }
    drop(repos_guard);

    items.sort_by(|a, b| {
        let ka = a["updated_at"].as_str().unwrap_or("");
        let kb = b["updated_at"].as_str().unwrap_or("");
        kb.cmp(ka)
    });

    Json(json!({
        "items": items,
        "default_branches": default_branches.into_iter().collect::<Vec<_>>(),
    }))
}

/// GET /api/v1/pr-insights/trend -- daily PR snapshot history.
///
/// Merges the per-repo `pr_snapshots` tables (written lazily by
/// `api_pr_insights`) into a single time series, summed across repos and
/// keyed by day. Empty until the dashboard has been viewed at least once;
/// one data point accumulates per day thereafter.
/// Parse the day (YYYY-MM-DD) out of an ISO8601 timestamp string.
fn trend_day(s: &str) -> Option<chrono::NaiveDate> {
    s.get(0..10)
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

/// Build a daily "open count" series from (created, closed?) intervals: an item
/// is open on day D if created <= D and (still open, or closed after D). Bounded
/// to the last 365 days; weekly buckets once the span exceeds ~180 days so the
/// point count stays reasonable. `key` names the value field ("open"/"total").
fn trend_series(
    intervals: &[(chrono::NaiveDate, Option<chrono::NaiveDate>)],
    key: &str,
) -> Vec<serde_json::Value> {
    if intervals.is_empty() {
        return Vec::new();
    }
    let today = chrono::Utc::now().date_naive();
    let min_created = intervals.iter().map(|(c, _)| *c).min().unwrap_or(today);
    let start = std::cmp::max(min_created, today - chrono::Duration::days(365));
    let span = (today - start).num_days().max(0);
    let step = if span > 180 { 7 } else { 1 };
    let mut points = Vec::new();
    let mut d = start;
    while d <= today {
        let open = intervals
            .iter()
            .filter(|(c, close)| *c <= d && close.is_none_or(|cl| cl > d))
            .count();
        let mut m = serde_json::Map::new();
        m.insert("day".to_string(), json!(d.format("%Y-%m-%d").to_string()));
        m.insert(key.to_string(), json!(open));
        points.push(serde_json::Value::Object(m));
        d += chrono::Duration::days(step);
    }
    points
}

/// GET /api/v1/pr-insights/trend -- open-PR history, reconstructed retroactively
/// from currently-open PRs' created_at, so it works on Postgres and shows data
/// immediately (no accumulated snapshots needed). We deliberately count only
/// OPEN PRs by creation date: `updated_at` reflects the last sync, not the
/// close time, so it can't reconstruct a true up/down open-count. The series is
/// therefore the cumulative build-up of today's open backlog and ends at the
/// real current open count.
async fn api_pr_trend(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    let repos: Vec<_> = {
        let g = state.multi.repos.read().await;
        g.values().cloned().collect()
    };
    let mut intervals: Vec<(chrono::NaiveDate, Option<chrono::NaiveDate>)> = Vec::new();
    for ds in &repos {
        for pr in ds.db.get_open_pulls().unwrap_or_default() {
            if let Some(created) = trend_day(&pr.created_at) {
                intervals.push((created, None));
            }
        }
    }
    Json(json!({ "points": trend_series(&intervals, "open") }))
}

/// GET /api/v1/issue-insights -- open-issue triage breakdown.
///
/// Returns every open issue across configured repos, each tagged with its
/// triage `category` / `priority` (from the `triage_results` table — null
/// when the issue has not been triaged yet) and a `triaged` flag. The SPA
/// derives the doughnut counts, the untriaged backlog and the label
/// histogram from this list.
async fn api_issue_insights(
    State(state): State<Arc<WebState>>,
    Query(filter): Query<RepoFilter>,
) -> impl IntoResponse {
    let mut items = Vec::new();

    let repos_guard = state.multi.repos.read().await;
    for (slug, ds) in repos_guard.iter() {
        let matches = filter.repo.as_ref().is_none_or(|f| f == slug);
        let Ok(issues) = ds.db.get_open_issues() else {
            continue;
        };

        // Resolve the triage row for each issue once; reused for both the
        // snapshot counts and the item list below.
        let mut triage_by_number = std::collections::HashMap::new();
        for issue in &issues {
            if let Ok(Some(t)) = ds.db.get_triage_result(issue.number) {
                triage_by_number.insert(issue.number, t);
            }
        }

        // (Trend history is reconstructed retroactively by api_issue_trend from
        // created_at, so no daily snapshot is recorded here.)

        if !matches {
            continue;
        }
        for issue in issues {
            let triage = triage_by_number.get(&issue.number);
            items.push(json!({
                "repo": slug,
                "number": issue.number,
                "title": issue.title,
                "author": issue.author,
                "labels": issue.labels,
                "category": triage.map(|t| t.category.as_str()),
                "priority": triage.and_then(|t| t.priority.as_deref()),
                "confidence": triage.map(|t| t.confidence),
                "summary": triage.and_then(|t| t.summary.as_deref()),
                "triaged": triage.is_some(),
                "created_at": issue.created_at,
                "updated_at": issue.updated_at,
                "url": crate::git_provider::web_url_for_issue(&ds.config, issue.number),
            }));
        }
    }
    drop(repos_guard);

    items.sort_by(|a, b| {
        let ka = a["updated_at"].as_str().unwrap_or("");
        let kb = b["updated_at"].as_str().unwrap_or("");
        kb.cmp(ka)
    });

    Json(json!({ "items": items }))
}

/// GET /api/v1/issue-insights/trend -- daily issue snapshot history.
///
/// Merges the per-repo `issue_snapshots` tables (written lazily by
/// `api_issue_insights`) into a single time series, summed across repos
/// and keyed by day. Empty until the dashboard has been viewed once.
/// GET /api/v1/issue-insights/trend -- open-issue history, reconstructed from
/// each open issue's created_at. Only currently-open issues are retained in the
/// DB, so this shows how the current backlog accumulated over time.
async fn api_issue_trend(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    let repos: Vec<_> = {
        let g = state.multi.repos.read().await;
        g.values().cloned().collect()
    };
    let mut intervals: Vec<(chrono::NaiveDate, Option<chrono::NaiveDate>)> = Vec::new();
    for ds in &repos {
        for issue in ds.db.get_open_issues().unwrap_or_default() {
            if let Some(created) = trend_day(&issue.created_at) {
                intervals.push((created, None));
            }
        }
    }
    Json(json!({ "points": trend_series(&intervals, "total") }))
}

// ---------------------------------------------------------------------------
// Usage dashboard (LLM accounting + GitHub rate limit)
// ---------------------------------------------------------------------------

/// GET /api/v1/usage/llm -- cross-repo LLM invocation counts.
///
/// Aggregates `UsageCounts` from every configured repo's backend and
/// rolls them up into a global total + per-repo + per-kind breakdown.
/// Empty/zero-default for OSS SQLite backends (the trait's default
/// returns zeroes) so the page degrades gracefully.
async fn api_usage_llm(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    let repos = state.multi.repos.read().await;
    let mut total: u64 = 0;
    let mut last_24h: u64 = 0;
    let mut last_7d: u64 = 0;
    let mut last_30d: u64 = 0;
    // (kind -> [total, 24h, 7d, 30d])
    let mut by_kind: std::collections::BTreeMap<String, [u64; 4]> =
        std::collections::BTreeMap::new();
    let mut by_repo: Vec<serde_json::Value> = Vec::with_capacity(repos.len());

    for (slug, ds) in repos.iter() {
        let counts = match ds.db.usage_counts() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("[{slug}] usage_counts failed: {e:#}");
                continue;
            }
        };
        total += counts.total;
        last_24h += counts.last_24h;
        last_7d += counts.last_7d;
        last_30d += counts.last_30d;
        for k in &counts.by_kind {
            let entry = by_kind.entry(k.kind.clone()).or_insert([0; 4]);
            entry[0] += k.total;
            entry[1] += k.last_24h;
            entry[2] += k.last_7d;
            entry[3] += k.last_30d;
        }
        by_repo.push(json!({
            "repo": slug,
            "total": counts.total,
            "last_24h": counts.last_24h,
            "last_7d": counts.last_7d,
            "last_30d": counts.last_30d,
        }));
    }
    drop(repos);

    let by_kind_json: Vec<serde_json::Value> = by_kind
        .into_iter()
        .map(|(k, v)| {
            json!({
                "kind": k,
                "total": v[0],
                "last_24h": v[1],
                "last_7d": v[2],
                "last_30d": v[3],
            })
        })
        .collect();

    Json(json!({
        "total": total,
        "last_24h": last_24h,
        "last_7d": last_7d,
        "last_30d": last_30d,
        "by_kind": by_kind_json,
        "by_repo": by_repo,
    }))
}

/// GET /api/v1/usage/github -- live GitHub REST rate limit + per-repo sync stats.
///
/// One unauthenticated REST call to `/rate_limit` (counted against the
/// same budget but trivial cost). Per-repo sync counts come from the
/// shared `sync_log` table via the existing `get_sync_entry` trait
/// (only the last-synced timestamp is exposed today — finer history
/// arrives in Step 2 of the dashboard).
async fn api_usage_github(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    // Live rate limit. We use anyhow-flavoured error handling so any
    // failure (network, auth, 403) degrades to nulls in the response
    // instead of erroring the whole page.
    let rate_limit = match fetch_github_rate_limit().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("github rate_limit fetch failed: {e:#}");
            json!({ "error": format!("{e:#}") })
        }
    };

    let repos = state.multi.repos.read().await;
    let mut per_repo: Vec<serde_json::Value> = Vec::with_capacity(repos.len());
    for (slug, ds) in repos.iter() {
        let issues = ds
            .db
            .get_sync_entry("issues")
            .ok()
            .flatten()
            .map(|e| e.last_synced_at);
        let pulls = ds
            .db
            .get_sync_entry("pulls")
            .ok()
            .flatten()
            .map(|e| e.last_synced_at);
        per_repo.push(json!({
            "repo": slug,
            "last_issue_sync": issues,
            "last_pull_sync": pulls,
        }));
    }
    drop(repos);

    Json(json!({
        "rate_limit": rate_limit,
        "by_repo": per_repo,
    }))
}

/// Live `GET https://api.github.com/rate_limit`. Best-effort: we send
/// the daemon's configured PAT if there is one (much higher limit),
/// otherwise fall back to anonymous.
async fn fetch_github_rate_limit() -> anyhow::Result<serde_json::Value> {
    let client = reqwest::Client::builder()
        .user_agent("wshm-usage-dashboard")
        .timeout(std::time::Duration::from_secs(8))
        .build()?;
    let mut req = client.get("https://api.github.com/rate_limit");
    if let Ok(tok) = std::env::var("GITHUB_TOKEN") {
        if !tok.is_empty() {
            req = req.bearer_auth(tok);
        }
    }
    let resp = req.send().await?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await?;
    if !status.is_success() {
        anyhow::bail!("github /rate_limit returned {status}: {body}");
    }
    Ok(body)
}

/// Feature API sub-router (review, conflicts, improvements, reports,
/// auto-fix candidates, search, insights, usage).
pub fn feature_api_routes() -> Router<Arc<WebState>> {
    Router::new()
        .route("/api/v1/reviews", get(api_reviews))
        .route("/api/v1/conflicts", get(api_conflicts))
        .route("/api/v1/improvements", get(api_improvements))
        .route("/api/v1/reports", get(api_reports))
        .route("/api/v1/autofix/candidates", get(api_autofix_candidates))
        .route("/api/v1/search", get(api_search))
        .route("/api/v1/pr-insights", get(api_pr_insights))
        .route("/api/v1/pr-insights/trend", get(api_pr_trend))
        .route("/api/v1/issue-insights", get(api_issue_insights))
        .route("/api/v1/issue-insights/trend", get(api_issue_trend))
        .route("/api/v1/usage/llm", get(api_usage_llm))
        .route("/api/v1/usage/github", get(api_usage_github))
}
