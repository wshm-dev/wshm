//! PR diagrams (`[pr.review_diagrams]`): an architecture graph of the PR,
//! drawn as Mermaid in the PR analysis comment and optionally published as
//! an interactive page.
//!
//! Flow: one AI call (after per-file summaries for large diffs) returns a
//! graph JSON, validated by [`graph::normalize`], cached per PR in
//! `pr_diagrams` and regenerated only when the diff or the generation
//! settings change. Everything is fail-soft: an error means no diagram, the
//! rest of the comment is unchanged.

pub mod diff_cache;
pub mod graph;
pub mod page;
pub mod publish;
pub mod render;

use anyhow::Result;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::ai::backend::AiBackend;
use crate::ai::prompts::pr_diagram;
use crate::config::{Config, DiagramPublishTarget, ReviewDiagramsConfig, DIAGRAM_KINDS};
use crate::db::backend::DatabaseBackend;
use crate::db::diagrams::PrDiagramRow;
use crate::db::pulls::PullRequest;
use crate::github::Client as GhClient;
use graph::Graph;

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Fingerprint of everything that changes the generated graph besides the diff.
fn gen_hash(dc: &ReviewDiagramsConfig) -> String {
    let mut kinds = dc.kinds.clone();
    kinds.sort();
    sha256_hex(&format!(
        "{}|{:?}|{}|{}",
        pr_diagram::PROMPT_VERSION,
        dc.text,
        dc.language.trim().to_ascii_lowercase(),
        kinds.join(",")
    ))
}

/// Lockfiles, generated and binary files: no architecture in there.
fn is_noise_path(path: &str) -> bool {
    const EXT: &[&str] = &[
        ".lock", ".sum", ".min.js", ".min.css", ".map", ".svg", ".png", ".jpg", ".jpeg", ".gif",
        ".ico", ".woff", ".woff2", ".ttf", ".eot", ".pdf", ".snap",
    ];
    const NAMES: &[&str] = &[
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "Cargo.lock",
        "go.sum",
        "composer.lock",
        "Gemfile.lock",
        "poetry.lock",
        "Pipfile.lock",
    ];
    NAMES.iter().any(|n| path.ends_with(n)) || EXT.iter().any(|e| path.ends_with(e))
}

/// Does the effective config ask for the architecture diagram?
fn wants_architecture(dc: &ReviewDiagramsConfig) -> bool {
    for k in &dc.kinds {
        if k != "architecture" {
            if DIAGRAM_KINDS.contains(&k.as_str()) {
                warn!("PR diagrams: kind '{k}' is not implemented yet, ignored");
            } else {
                warn!("PR diagrams: unknown kind '{k}', ignored");
            }
        }
    }
    dc.kinds.iter().any(|k| k == "architecture")
}

#[derive(Deserialize, Default)]
struct FileSummaries {
    #[serde(default)]
    files: Vec<FileSummary>,
}

#[derive(Deserialize, Default)]
struct FileSummary {
    #[serde(default)]
    path: String,
    #[serde(default)]
    summary: String,
}

fn added_deleted(chunk: &str) -> (usize, usize) {
    let mut add = 0;
    let mut del = 0;
    for l in chunk.lines() {
        if l.starts_with('+') && !l.starts_with("+++") {
            add += 1;
        } else if l.starts_with('-') && !l.starts_with("---") {
            del += 1;
        }
    }
    (add, del)
}

/// User prompt of the graph call: the diff itself, or per-file summaries
/// when it is larger than [`pr_diagram::MAX_DIRECT_DIFF_BYTES`].
async fn graph_user_prompt(
    ai: &AiBackend,
    db: &dyn DatabaseBackend,
    pr: &PullRequest,
    diff: &str,
) -> String {
    let body = pr.body.as_deref().unwrap_or("(no description)");
    let chunks: Vec<(String, String)> = crate::ai::prompts::inline_review::split_diff_by_file(diff)
        .into_iter()
        .filter(|(p, _)| !is_noise_path(p))
        .collect();
    let useful: usize = chunks.iter().map(|(_, c)| c.len()).sum();
    if useful <= pr_diagram::MAX_DIRECT_DIFF_BYTES {
        let joined: String = chunks.iter().map(|(_, c)| c.as_str()).collect();
        return pr_diagram::graph_user_from_diff(&pr.title, body, &joined);
    }

    // Batch files into calls of at most SUMMARY_BATCH_BYTES each.
    let mut batches: Vec<Vec<&(String, String)>> = Vec::new();
    let mut size = 0;
    for c in &chunks {
        let len = c.1.len().min(pr_diagram::SUMMARY_BATCH_BYTES);
        if batches.is_empty() || size + len > pr_diagram::SUMMARY_BATCH_BYTES {
            batches.push(Vec::new());
            size = 0;
        }
        batches.last_mut().expect("just pushed").push(c);
        size += len;
    }
    info!(
        "PR #{} diagram: diff of {} KB (without lockfiles/generated files) > {} KB, summarizing {} files in {} call(s) first",
        pr.number,
        useful / 1024,
        pr_diagram::MAX_DIRECT_DIFF_BYTES / 1000,
        chunks.len(),
        batches.len().min(pr_diagram::MAX_SUMMARY_CALLS)
    );
    let mut summaries: std::collections::HashMap<String, String> = Default::default();
    for batch in batches.iter().take(pr_diagram::MAX_SUMMARY_CALLS) {
        let user = pr_diagram::summary_user(&pr.title, batch);
        match ai
            .complete::<FileSummaries>(pr_diagram::SUMMARY_SYSTEM, &user)
            .await
        {
            Ok(s) => {
                let _ = db.record_llm_invocation("pr_diagram", None);
                for f in s.files {
                    summaries.insert(
                        f.path.trim().to_string(),
                        graph::clean_text(&f.summary, 600),
                    );
                }
            }
            Err(e) => warn!("PR diagram: file summary call failed, continuing: {e:#}"),
        }
    }
    if batches.len() > pr_diagram::MAX_SUMMARY_CALLS {
        info!(
            "PR diagram: {} file batches, only {} summarized",
            batches.len(),
            pr_diagram::MAX_SUMMARY_CALLS
        );
    }
    let files: Vec<(String, usize, usize, Option<String>)> = chunks
        .iter()
        .map(|(p, c)| {
            let (a, d) = added_deleted(c);
            (p.clone(), a, d, summaries.get(p).cloned())
        })
        .collect();
    pr_diagram::graph_user_from_summaries(&pr.title, body, &files)
}

/// Ask the model for the graph and validate it.
async fn generate(
    ai: &AiBackend,
    db: &dyn DatabaseBackend,
    pr: &PullRequest,
    diff: &str,
    dc: &ReviewDiagramsConfig,
) -> Result<Option<Graph>> {
    let mut user = graph_user_prompt(ai, db, pr, diff).await;
    let skills = crate::config::load_skills(db);
    let skills_prompt = crate::config::skills_prompt(&skills, "diagrams");
    if !skills_prompt.is_empty() {
        user.push_str(&skills_prompt);
        user.push_str(
            "\n(For this diagram, apply only the instructions about how to describe or name the code; ignore code-review and bug-finding instructions.)\n",
        );
    }
    let system = pr_diagram::graph_system(&dc.language, dc.text);
    let raw: serde_json::Value = ai.complete(&system, &user).await?;
    let _ = db.record_llm_invocation("pr_diagram", None);
    let g = graph::normalize(&raw, &graph::diff_paths(diff), dc.text);
    if g.is_none() {
        warn!(
            "PR #{}: the diagram model output had nothing drawable",
            pr.number
        );
    }
    Ok(g)
}

/// Build the "Architecture" section for the PR analysis comment. Fail-soft:
/// any error is logged and yields `None`.
pub async fn comment_section(
    config: &Config,
    ai: &AiBackend,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    pr: &PullRequest,
    diff: Option<&str>,
    apply: bool,
) -> Option<String> {
    match build_section(config, ai, db, gh, pr, diff, apply).await {
        Ok(s) => s,
        Err(e) => {
            warn!("PR #{}: diagram skipped: {e:#}", pr.number);
            None
        }
    }
}

/// Single hook point for the forge capability: whether PR comments render
/// Mermaid. GitHub, GitLab, Gitea/Forgejo do; Azure DevOps does not. Until
/// the multi-forge work (wshm#227) exposes `supports_mermaid()` on the
/// provider, every repo here is a GitHub repo, hence `true`. When it is
/// false, the comment shows the page link instead of the diagram.
pub fn forge_supports_mermaid(_config: &Config) -> bool {
    true
}

/// Is the PR recent enough to (re)generate its diagram? `updated_at` moves
/// on every push, so a new push re-enables an idle PR. 0 = no limit; an
/// unparseable date counts as active.
pub fn is_active(updated_at: &str, now: chrono::DateTime<chrono::Utc>, active_days: u32) -> bool {
    if active_days == 0 {
        return true;
    }
    match chrono::DateTime::parse_from_rfc3339(updated_at) {
        Ok(t) => now.signed_duration_since(t) <= chrono::Duration::days(active_days as i64),
        Err(_) => true,
    }
}

/// PRs whose page must go so that at most `max_pages` remain: the least
/// recently updated ones first (ties: lowest PR number first).
pub fn pages_to_evict(rows: &[PrDiagramRow], max_pages: usize) -> Vec<u64> {
    let mut with_page: Vec<&PrDiagramRow> =
        rows.iter().filter(|r| r.page_target.is_some()).collect();
    if with_page.len() <= max_pages {
        return Vec::new();
    }
    with_page.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then(b.pr_number.cmp(&a.pr_number))
    });
    let mut out: Vec<u64> = with_page[max_pages..].iter().map(|r| r.pr_number).collect();
    out.sort_unstable();
    out
}

/// Has the page not been updated for `retention_days`? (0 = never expires)
pub fn page_expired(
    row: &PrDiagramRow,
    now: chrono::DateTime<chrono::Utc>,
    retention_days: u32,
) -> bool {
    retention_days > 0
        && row.page_target.is_some()
        && chrono::DateTime::parse_from_rfc3339(&row.updated_at)
            .map(|t| now.signed_duration_since(t) > chrono::Duration::days(retention_days as i64))
            .unwrap_or(false)
}

/// Dashboard numbers of one repo.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize)]
pub struct DiagramStats {
    /// Cached graphs (rows).
    pub graphs: u64,
    /// Published pages.
    pub pages: u64,
    /// Bytes kept in the DB (graph JSON).
    pub db_bytes: u64,
    /// Total size of the published pages (last render of each).
    pub page_bytes: u64,
}

pub fn stats(rows: &[PrDiagramRow]) -> DiagramStats {
    let mut s = DiagramStats::default();
    for r in rows {
        s.graphs += 1;
        s.db_bytes += r.graph_json.len() as u64;
        if r.page_target.is_some() {
            s.pages += 1;
            s.page_bytes += r.page_bytes;
        }
    }
    s
}

/// Fingerprint of what a static page depends on besides the graph and diff.
pub fn page_fingerprint(dc: &ReviewDiagramsConfig) -> String {
    sha256_hex(&format!(
        "{}|{}|{}|{}",
        page::template_hash(),
        dc.language,
        dc.diff_viewer,
        dc.publish.max_diff_kb
    ))[..16]
        .to_string()
}

fn clear_page(row: &mut PrDiagramRow) {
    row.page_target = None;
    row.page_key = None;
    row.page_url = None;
    row.page_token = None;
    row.page_bytes = 0;
    row.page_hash = None;
}

/// Delete the published page of `row` and forget it (the graph stays cached).
async fn evict_page(
    dc: &ReviewDiagramsConfig,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    row: &PrDiagramRow,
    why: &str,
) {
    if let Err(e) = publish::delete_page(&dc.publish, gh, row).await {
        warn!(
            "PR #{}: {why} diagram page cleanup failed: {e:#}",
            row.pr_number
        );
        return;
    }
    if let Ok(Some(mut full)) = db.get_pr_diagram(row.pr_number) {
        clear_page(&mut full);
        let _ = db.upsert_pr_diagram(&full);
    }
    info!("PR #{}: {why} diagram page removed", row.pr_number);
}

/// Enforce `publish.max_pages` for the repo.
async fn enforce_max_pages(dc: &ReviewDiagramsConfig, db: &dyn DatabaseBackend, gh: &GhClient) {
    let Ok(rows) = db.list_pr_diagrams() else {
        return;
    };
    for n in pages_to_evict(&rows, dc.publish.max_pages as usize) {
        if let Some(row) = rows.iter().find(|r| r.pr_number == n) {
            evict_page(dc, db, gh, row, "evicted (max_pages)").await;
        }
    }
}

/// ETag of a hosted page. The page is a pure function of these inputs, so a
/// revalidation is answered without fetching the diff.
pub fn hosted_etag(row: &PrDiagramRow, dc: &ReviewDiagramsConfig) -> String {
    let h = sha256_hex(&format!(
        "{}|{}|{}|{:?}|{}|{}|{}",
        page::template_hash(),
        row.graph_json,
        row.diff_hash,
        row.head_sha,
        dc.language,
        dc.diff_viewer,
        dc.publish.max_diff_kb
    ));
    format!("\"{}\"", &h[..32])
}

/// Does an `If-None-Match` header match `etag`? (list, weak tags and `*`)
pub fn etag_matches(if_none_match: Option<&str>, etag: &str) -> bool {
    if_none_match.is_some_and(|v| {
        v.split(',').any(|t| {
            let t = t.trim();
            t == "*" || t == etag || t.strip_prefix("W/") == Some(etag)
        })
    })
}

/// Render the interactive page of `graph`. The diff is embedded only when
/// allowed and small enough, otherwise the viewer links to "Files changed".
pub fn render_page(
    dc: &ReviewDiagramsConfig,
    repo_slug: &str,
    pr: u64,
    graph: &Graph,
    diff: Option<&str>,
) -> String {
    let embedded =
        diff.and_then(|d| page::embeddable_diff(d, dc.diff_viewer, dc.publish.max_diff_kb));
    let files = page::files_url(repo_slug, pr);
    page::render(
        graph,
        &page::PageOptions {
            language: &dc.language,
            repo: repo_slug,
            pr,
            diff: embedded,
            files_url: embedded.is_none().then_some(files.as_str()),
        },
    )
}

async fn build_section(
    config: &Config,
    ai: &AiBackend,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    pr: &PullRequest,
    diff: Option<&str>,
    apply: bool,
) -> Result<Option<String>> {
    let dc = crate::config::load_review_diagrams(config, db);
    if !dc.enabled || !wants_architecture(&dc) {
        return Ok(None);
    }
    if pr.state != "open" {
        return Ok(None);
    }
    let Some(diff) = diff.filter(|d| !d.trim().is_empty()) else {
        return Ok(None);
    };
    let diff_hash = sha256_hex(diff);
    let gen = gen_hash(&dc);
    let previous = db.get_pr_diagram(pr.number).ok().flatten();
    let target = publish::target_name(dc.publish.target);
    let repo_slug = config.repo_slug();

    let cached = previous
        .as_ref()
        .filter(|p| p.diff_hash == diff_hash && p.gen_hash == gen)
        .and_then(|p| serde_json::from_str::<Graph>(&p.graph_json).ok());
    let fresh = cached.is_none();
    let graph = match cached {
        Some(g) => {
            info!(
                "PR #{}: diagram unchanged (diff hash match), reusing it",
                pr.number
            );
            g
        }
        None if !is_active(&pr.updated_at, chrono::Utc::now(), dc.active_days) => {
            info!(
                "PR #{}: no activity for more than {} days, no diagram generated",
                pr.number, dc.active_days
            );
            return Ok(None);
        }
        None => match generate(ai, db, pr, diff, &dc).await? {
            Some(g) => g,
            None => return Ok(None),
        },
    };

    let now = chrono::Utc::now().to_rfc3339();
    let mut row = PrDiagramRow {
        pr_number: pr.number,
        diff_hash,
        head_sha: pr.head_sha.clone(),
        gen_hash: gen,
        graph_json: serde_json::to_string(&graph)?,
        created_at: previous
            .as_ref()
            .map(|p| p.created_at.clone())
            .unwrap_or_else(|| now.clone()),
        updated_at: now,
        ..Default::default()
    };

    // A page published to another target before (config changed): remove it.
    if let Some(p) = previous.as_ref() {
        if p.page_target.is_some() && p.page_target.as_deref() != Some(target) && apply {
            if let Err(e) = publish::delete_page(&dc.publish, gh, p).await {
                warn!(
                    "PR #{}: could not delete the previous diagram page: {e:#}",
                    pr.number
                );
            }
        }
    }

    // Hosted pages are rendered on request; static files must match the
    // current rendering settings too.
    let page_fp = page_fingerprint(&dc);
    let page_ok = previous.as_ref().filter(|p| {
        !fresh
            && p.page_target.as_deref() == Some(target)
            && p.page_key.is_some()
            && (target == "wshm" || p.page_hash.as_deref() == Some(page_fp.as_str()))
    });
    // Dry-run never deletes: keep tracking a page published elsewhere so a
    // later real run (or the close cleanup) can still remove it.
    let keep_foreign = previous
        .as_ref()
        .filter(|p| !apply && p.page_target.is_some() && p.page_target.as_deref() != Some(target));
    if let Some(p) = page_ok.or(keep_foreign) {
        // Same diff, same target: the published page is still current.
        row.page_target = p.page_target.clone();
        row.page_key = p.page_key.clone();
        row.page_url = p.page_url.clone();
        row.page_token = p.page_token.clone();
        row.page_bytes = p.page_bytes;
        row.page_hash = p.page_hash.clone();
        row.is_private = p.is_private;
    } else if dc.publish.target != DiagramPublishTarget::Off {
        let is_private = match gh.fetch_repo_private().await {
            Ok(p) => p,
            Err(e) => {
                warn!("Could not read the visibility of {repo_slug}, assuming private: {e:#}");
                true
            }
        };
        let html = render_page(&dc, &repo_slug, pr.number, &graph, Some(diff));
        match publish::publish(
            &dc.publish,
            gh,
            &repo_slug,
            pr.number,
            &html,
            is_private,
            previous.as_ref(),
            !apply,
        )
        .await
        {
            Ok(Some(p)) => {
                row.page_target = Some(p.target.to_string());
                row.page_key = Some(p.key);
                row.page_url = p.url;
                row.page_token = p.token;
                row.page_bytes = p.bytes;
                row.page_hash = Some(page_fp.clone());
                row.is_private = is_private;
            }
            Ok(None) => {}
            Err(e) => warn!("PR #{}: diagram page not published: {e:#}", pr.number),
        }
    }
    db.upsert_pr_diagram(&row)?;
    if apply && row.page_target.is_some() {
        enforce_max_pages(&dc, db, gh).await;
    }
    // Eviction may have removed this very page.
    let page_url = db
        .get_pr_diagram(pr.number)
        .ok()
        .flatten()
        .and_then(|r| r.page_url)
        .or(if apply { None } else { row.page_url.clone() });

    Ok(render::comment_section(
        &graph,
        &render::SectionOptions {
            language: &dc.language,
            text: dc.text,
            comment: dc.comment,
            page_url: page_url.as_deref(),
            page_has_diff: dc.diff_viewer && diff.len() <= dc.publish.max_diff_kb as usize * 1024,
            max_mermaid_chars: dc.max_mermaid_chars as usize,
            supports_mermaid: forge_supports_mermaid(config),
        },
    ))
}

/// Delete the published page and the cached graph of a PR (closed/merged).
/// Fail-soft.
pub async fn cleanup_pr(config: &Config, db: &dyn DatabaseBackend, gh: &GhClient, pr_number: u64) {
    let Ok(Some(row)) = db.get_pr_diagram(pr_number) else {
        return;
    };
    let dc = crate::config::load_review_diagrams(config, db);
    if let Err(e) = publish::delete_page(&dc.publish, gh, &row).await {
        warn!("PR #{pr_number}: diagram page cleanup failed: {e:#}");
    }
    match db.delete_pr_diagram(pr_number) {
        Ok(()) => info!("PR #{pr_number}: diagram removed"),
        Err(e) => warn!("PR #{pr_number}: could not delete the cached diagram: {e:#}"),
    }
}

/// Cleanup sweep for the polling mode (no `closed` webhook): drop diagrams
/// of PRs no longer open, pages idle for `retention_days`, and pages beyond
/// `max_pages`.
pub async fn sweep(config: &Config, db: &dyn DatabaseBackend, gh: &GhClient) {
    let Ok(rows) = db.list_pr_diagrams() else {
        return;
    };
    if rows.is_empty() {
        return;
    }
    let dc = crate::config::load_review_diagrams(config, db);
    let now = chrono::Utc::now();
    for row in &rows {
        let open = matches!(db.get_pull(row.pr_number), Ok(Some(p)) if p.state == "open");
        if !open {
            cleanup_pr(config, db, gh, row.pr_number).await;
        } else if page_expired(row, now, dc.publish.retention_days) {
            evict_page(&dc, db, gh, row, "expired (retention_days)").await;
        }
    }
    enforce_max_pages(&dc, db, gh).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_hash_tracks_generation_settings_only() {
        let a = ReviewDiagramsConfig::default();
        let mut b = a.clone();
        b.publish.target = DiagramPublishTarget::Local;
        b.diff_viewer = false;
        assert_eq!(
            gen_hash(&a),
            gen_hash(&b),
            "publish settings do not regenerate"
        );
        b.language = "fr".into();
        assert_ne!(gen_hash(&a), gen_hash(&b));
        let mut c = a.clone();
        c.text = crate::config::DiagramText::Full;
        assert_ne!(gen_hash(&a), gen_hash(&c));
    }

    #[test]
    fn noise_paths() {
        assert!(is_noise_path("Cargo.lock"));
        assert!(is_noise_path("web/package-lock.json"));
        assert!(!is_noise_path("src/main.rs"));
    }

    #[test]
    fn kinds() {
        let mut dc = ReviewDiagramsConfig::default();
        assert!(wants_architecture(&dc));
        dc.kinds = vec!["flow".into(), "bogus".into()];
        assert!(!wants_architecture(&dc));
    }

    fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().into()
    }

    fn row(n: u64, updated: &str, page: bool) -> PrDiagramRow {
        PrDiagramRow {
            pr_number: n,
            graph_json: "{}".into(),
            page_target: page.then(|| "wshm".to_string()),
            page_bytes: if page { 100 } else { 0 },
            updated_at: updated.into(),
            ..Default::default()
        }
    }

    #[test]
    fn activity_filter() {
        let now = at("2026-10-20T00:00:00Z");
        assert!(is_active("2026-10-10T00:00:00Z", now, 14));
        assert!(
            !is_active("2026-10-05T23:59:59Z", now, 14),
            "idle 14 days and more"
        );
        assert!(is_active("2020-01-01T00:00:00Z", now, 0), "0 = no limit");
        assert!(is_active("garbage", now, 14));
        // A push moves updated_at: the PR is active again.
        assert!(is_active("2026-10-19T12:00:00Z", now, 14));
    }

    #[test]
    fn eviction_keeps_the_most_recently_updated_pages() {
        let rows = vec![
            row(1, "2026-10-01T00:00:00Z", true),
            row(2, "2026-10-03T00:00:00Z", true),
            row(3, "2026-10-02T00:00:00Z", true),
            row(4, "2026-09-01T00:00:00Z", false),
            row(5, "2026-10-04T00:00:00Z", true),
        ];
        assert!(pages_to_evict(&rows, 4).is_empty());
        assert_eq!(pages_to_evict(&rows, 2), vec![1, 3]);
        assert_eq!(pages_to_evict(&rows, 0), vec![1, 2, 3, 5]);
    }

    #[test]
    fn retention() {
        let now = at("2026-10-31T00:00:00Z");
        assert!(page_expired(&row(1, "2026-09-30T00:00:00Z", true), now, 30));
        assert!(!page_expired(
            &row(1, "2026-10-02T00:00:00Z", true),
            now,
            30
        ));
        assert!(
            !page_expired(&row(1, "2026-01-01T00:00:00Z", false), now, 30),
            "no page"
        );
        assert!(!page_expired(&row(1, "2026-01-01T00:00:00Z", true), now, 0));
    }

    #[test]
    fn dashboard_stats() {
        let rows = vec![row(1, "x", true), row(2, "x", false), row(3, "x", true)];
        assert_eq!(
            stats(&rows),
            DiagramStats {
                graphs: 3,
                pages: 2,
                db_bytes: 6,
                page_bytes: 200
            }
        );
    }

    #[test]
    fn etags() {
        let dc = ReviewDiagramsConfig::default();
        let r = row(1, "x", true);
        let e = hosted_etag(&r, &dc);
        assert!(e.starts_with('"') && e.ends_with('"') && e.len() == 34);
        assert_eq!(e, hosted_etag(&r, &dc), "stable");
        let mut r2 = r.clone();
        r2.diff_hash = "other".into();
        assert_ne!(e, hosted_etag(&r2, &dc), "new diff, new etag");
        let mut dc2 = dc.clone();
        dc2.language = "fr".into();
        assert_ne!(e, hosted_etag(&r, &dc2));
        assert!(etag_matches(Some(&e), &e));
        assert!(etag_matches(Some(&format!("\"a\", W/{e}")), &e));
        assert!(etag_matches(Some("*"), &e));
        assert!(!etag_matches(Some("\"a\""), &e));
        assert!(!etag_matches(None, &e));
    }

    #[test]
    fn new_defaults() {
        let dc = ReviewDiagramsConfig::default();
        assert_eq!(dc.comment, crate::config::DiagramComment::Mermaid);
        assert_eq!(dc.publish.target, DiagramPublishTarget::Off);
        assert_eq!(
            (
                dc.active_days,
                dc.max_mermaid_chars,
                dc.publish.retention_days,
                dc.publish.max_pages,
                dc.publish.max_diff_kb
            ),
            (14, 5000, 30, 200, 1024)
        );
    }
}
