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
use crate::git_provider::Forge as GhClient;
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
    max_calls: usize,
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
        batches.len().min(max_calls)
    );
    let mut summaries: std::collections::HashMap<String, String> = Default::default();
    for batch in batches.iter().take(max_calls) {
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
    if batches.len() > max_calls {
        info!(
            "PR diagram: {} file batches, only {} summarized",
            batches.len(),
            max_calls
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
    let mut user = graph_user_prompt(ai, db, pr, diff, dc.max_summary_calls as usize).await;
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

fn section_options<'a>(
    gh: &GhClient,
    dc: &'a ReviewDiagramsConfig,
    page_url: Option<&'a str>,
    diff_len: usize,
) -> render::SectionOptions<'a> {
    render::SectionOptions {
        language: &dc.language,
        text: dc.text,
        comment: dc.comment,
        page_url,
        page_has_diff: dc.diff_viewer && diff_len <= dc.publish.max_diff_kb as usize * 1024,
        max_mermaid_chars: dc.max_mermaid_chars as usize,
        supports_mermaid: forge_supports_mermaid(gh),
    }
}

/// The section of an already generated diagram for this exact diff, from the
/// DB alone (no AI, no network). Lets the analysis comment carry an
/// unchanged diagram right away instead of waiting for the diagram step.
pub fn cached_section(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    pr: &PullRequest,
    diff: Option<&str>,
) -> Option<String> {
    let dc = crate::config::load_review_diagrams(config, db);
    if !dc.enabled || !dc.kinds.iter().any(|k| k == "architecture") {
        return None;
    }
    let diff = diff.filter(|d| !d.trim().is_empty())?;
    let row = db.get_pr_diagram(pr.number).ok().flatten()?;
    if row.diff_hash != sha256_hex(diff) || row.gen_hash != gen_hash(&dc) {
        return None;
    }
    let graph: Graph = serde_json::from_str(&row.graph_json).ok()?;
    render::comment_section(
        &graph,
        &section_options(gh, &dc, row.page_url.as_deref(), diff.len()),
    )
}

/// The diagram step: generate (or reuse) the graph, publish the page and
/// build the "Architecture" section. Bounded by `timeout_secs` (on expiry
/// the futures are dropped, which kills the AI CLI) and fail-soft: any
/// error or timeout is logged and yields `None`.
pub async fn comment_section(
    config: &Config,
    ai: &AiBackend,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    pr: &PullRequest,
    diff: Option<&str>,
    apply: bool,
) -> Option<String> {
    let budget = crate::config::load_review_diagrams(config, db)
        .timeout_secs
        .max(1);
    match tokio::time::timeout(
        std::time::Duration::from_secs(budget as u64),
        build_section(config, ai, db, gh, pr, diff, apply),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            warn!("PR #{}: diagram skipped: {e:#}", pr.number);
            None
        }
        Err(_) => {
            warn!(
                "PR #{}: diagram step abandoned after {budget}s (timeout_secs)",
                pr.number
            );
            None
        }
    }
}

/// Single hook point for the forge capability: whether PR comments render
/// Mermaid (GitHub, GitLab, Gitea/Forgejo yes; Azure DevOps no). When it is
/// false the comment links to the interactive page if one is published,
/// otherwise it keeps the story and walkthrough text only.
pub fn forge_supports_mermaid(gh: &GhClient) -> bool {
    gh.supports_mermaid()
}

/// "Files changed" view of a PR on the configured forge.
pub fn files_url(config: &Config, pr: u64) -> Option<String> {
    let url = crate::git_provider::web_url_for_pr(config, pr)?;
    Some(match crate::git_provider::provider_id(config) {
        "gitlab" => format!("{url}/diffs"),
        "azure-devops" | "azure" => format!("{url}?_a=files"),
        _ => format!("{url}/files"),
    })
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

/// Repo visibility for the hosted pages, from the forge, cached 5 minutes
/// per repo. Unknown (error) counts as private.
pub async fn repo_is_private(slug: &str, gh: &GhClient) -> bool {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, (bool, Instant)>>> =
        std::sync::OnceLock::new();
    const TTL: Duration = Duration::from_secs(300);
    let cache = CACHE.get_or_init(Default::default);
    if let Some((v, at)) = cache.lock().ok().and_then(|c| c.get(slug).copied()) {
        if at.elapsed() < TTL {
            return v;
        }
    }
    let v = gh.fetch_repo_private().await.unwrap_or(true);
    if let Ok(mut c) = cache.lock() {
        if c.len() > 1000 {
            c.clear();
        }
        c.insert(slug.to_string(), (v, Instant::now()));
    }
    v
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
/// `current_head` is the PR head now (the embedded diff is fetched for it).
pub fn hosted_etag(
    row: &PrDiagramRow,
    dc: &ReviewDiagramsConfig,
    current_head: Option<&str>,
    is_private: bool,
) -> String {
    let h = sha256_hex(&format!(
        "{}|{}|{}|{:?}|{current_head:?}|{is_private}|{}|{}|{}",
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
    config: &Config,
    pr: u64,
    graph: &Graph,
    diff: Option<&str>,
    assets: page::Assets,
) -> String {
    let embedded =
        diff.and_then(|d| page::embeddable_diff(d, dc.diff_viewer, dc.publish.max_diff_kb));
    let repo_slug = config.repo_slug();
    let files = files_url(config, pr);
    page::render(
        graph,
        &page::PageOptions {
            language: &dc.language,
            repo: &repo_slug,
            pr,
            diff: embedded,
            files_url: if embedded.is_none() {
                files.as_deref()
            } else {
                None
            },
            assets,
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
        // Hosted pages load the vendored assets from the daemon; files
        // published elsewhere are self-contained.
        let assets = if dc.publish.target == DiagramPublishTarget::Wshm {
            page::Assets::Hosted
        } else {
            page::Assets::Inline
        };
        let html = render_page(&dc, config, pr.number, &graph, Some(diff), assets);
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
            Ok(None) => keep_previous_page(&mut row, previous.as_ref(), target, apply),
            Err(e) => {
                warn!("PR #{}: diagram page not published: {e:#}", pr.number);
                keep_previous_page(&mut row, previous.as_ref(), target, apply);
            }
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
        &section_options(gh, &dc, page_url.as_deref(), diff.len()),
    ))
}

/// Publication skipped (dry-run, refused) or failed: keep tracking the page
/// published before, so its link (already in the comment) keeps working and
/// a later cleanup can still delete it. Not when a real run just deleted
/// it because the target changed.
fn keep_previous_page(
    row: &mut PrDiagramRow,
    previous: Option<&PrDiagramRow>,
    target: &str,
    apply: bool,
) {
    let Some(p) = previous.filter(|p| p.page_target.is_some()) else {
        return;
    };
    if apply && p.page_target.as_deref() != Some(target) {
        return;
    }
    row.page_target = p.page_target.clone();
    row.page_key = p.page_key.clone();
    row.page_url = p.page_url.clone();
    row.page_token = p.page_token.clone();
    row.page_bytes = p.page_bytes;
    row.page_hash = p.page_hash.clone();
    row.is_private = p.is_private;
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
    fn previous_page_survives_a_skipped_or_failed_publication() {
        let prev = PrDiagramRow {
            page_target: Some("github-pages".into()),
            page_key: Some("k".into()),
            page_url: Some("https://x/k".into()),
            page_token: Some("t".into()),
            page_bytes: 9,
            page_hash: Some("h".into()),
            ..Default::default()
        };
        let mut row = PrDiagramRow::default();
        keep_previous_page(&mut row, Some(&prev), "github-pages", true);
        assert_eq!(row.page_url.as_deref(), Some("https://x/k"));
        assert_eq!(row.page_token.as_deref(), Some("t"));
        // Dry-run with another target: still tracked (nothing was deleted).
        let mut row = PrDiagramRow::default();
        keep_previous_page(&mut row, Some(&prev), "wshm", false);
        assert_eq!(row.page_key.as_deref(), Some("k"));
        // Real run that switched target: the old page was deleted, forget it.
        let mut row = PrDiagramRow::default();
        keep_previous_page(&mut row, Some(&prev), "wshm", true);
        assert!(row.page_key.is_none());
        let mut row = PrDiagramRow::default();
        keep_previous_page(&mut row, None, "wshm", true);
        assert!(row.page_target.is_none());
    }

    fn pull(n: u64, state: &str) -> PullRequest {
        PullRequest {
            number: n,
            title: format!("PR {n}"),
            body: None,
            state: state.into(),
            labels: vec![],
            author: None,
            head_sha: Some("h".into()),
            base_sha: None,
            head_ref: None,
            base_ref: None,
            mergeable: None,
            ci_status: None,
            created_at: "2026-10-01T00:00:00Z".into(),
            updated_at: "2026-10-01T00:00:00Z".into(),
            review_decision: None,
            review_decision_at: None,
            reactions_plus1: 0,
        }
    }

    #[tokio::test]
    async fn sweep_drops_closed_prs_and_expired_pages() {
        let db = crate::db::Database::open_memory().unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = Config::default();
        let gh = crate::git_provider::github::GitHubProvider::new(&config).unwrap();
        db.upsert_pull(&pull(1, "open")).unwrap();
        db.upsert_pull(&pull(2, "closed")).unwrap();
        db.upsert_pull(&pull(3, "open")).unwrap();
        let fresh = chrono::Utc::now().to_rfc3339();
        // 1: open, recent page -> kept. 2: closed (e.g. re-created by an
        // analysis that raced the close) -> removed. 3: open, page idle for
        // 40 days -> page dropped, graph kept. 4: PR unknown -> removed.
        db.upsert_pr_diagram(&row(1, &fresh, true)).unwrap();
        db.upsert_pr_diagram(&row(2, &fresh, true)).unwrap();
        db.upsert_pr_diagram(&row(3, "2026-01-01T00:00:00Z", true))
            .unwrap();
        db.upsert_pr_diagram(&row(4, &fresh, false)).unwrap();
        sweep(&config, &db, &gh).await;
        assert!(db.get_pr_diagram(1).unwrap().unwrap().page_target.is_some());
        assert!(db.get_pr_diagram(2).unwrap().is_none());
        let r3 = db.get_pr_diagram(3).unwrap().unwrap();
        assert!(r3.page_target.is_none() && r3.page_url.is_none());
        assert!(db.get_pr_diagram(4).unwrap().is_none());
    }

    #[test]
    fn mermaid_follows_the_forge_and_azure_gets_a_link() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let graph: Graph = serde_json::from_value(serde_json::json!({
            "title": "t", "story": "Story.", "layers": ["A", "B"],
            "nodes": [
                {"id": "a", "label": "A", "file": "a.rs", "layer": 0, "status": "modified", "summary": "s"},
                {"id": "b", "label": "B", "file": "b.rs", "layer": 1, "status": "new", "summary": "s"}
            ],
            "edges": [{"source": "a", "target": "b", "label": "call", "step": 1, "status": "modified", "caption": "c"}]
        }))
        .unwrap();
        let dc = ReviewDiagramsConfig::default();
        for (id, mermaid, files) in [
            (
                "gitlab",
                true,
                "https://gitlab.com/acme/proj/repo/-/merge_requests/9/diffs",
            ),
            (
                "gitea",
                true,
                "https://gitea.com/acme/proj/repo/pulls/9/files",
            ),
            (
                "azure-devops",
                false,
                "https://dev.azure.com/acme/proj/_git/repo/pullrequest/9?_a=files",
            ),
        ] {
            let config = Config {
                git_provider: Some(id.into()),
                repo_owner: "acme".into(),
                repo_name: "proj/repo".into(),
                ..Default::default()
            };
            let gh = crate::git_provider::build_provider(&config).unwrap();
            assert_eq!(forge_supports_mermaid(gh.as_ref()), mermaid, "{id}");
            assert_eq!(files_url(&config, 9).as_deref(), Some(files), "{id}");
            for url in [Some("https://pages.example/p.html"), None] {
                let s =
                    render::comment_section(&graph, &section_options(gh.as_ref(), &dc, url, 10))
                        .unwrap();
                assert_eq!(s.contains("```mermaid"), mermaid, "{id}");
                assert_eq!(
                    s.contains("](https://pages.example/p.html)"),
                    url.is_some(),
                    "{id}"
                );
                assert!(
                    s.contains("Story.") && s.contains("<b>Walkthrough</b>"),
                    "{id}"
                );
            }
        }
    }

    #[test]
    fn etags() {
        let dc = ReviewDiagramsConfig::default();
        let r = row(1, "x", true);
        let e = hosted_etag(&r, &dc, Some("h1"), false);
        assert!(e.starts_with('"') && e.ends_with('"') && e.len() == 34);
        assert_eq!(e, hosted_etag(&r, &dc, Some("h1"), false), "stable");
        let mut r2 = r.clone();
        r2.diff_hash = "other".into();
        assert_ne!(
            e,
            hosted_etag(&r2, &dc, Some("h1"), false),
            "new diff, new etag"
        );
        assert_ne!(
            e,
            hosted_etag(&r, &dc, Some("h2"), false),
            "new head, new etag"
        );
        let mut dc2 = dc.clone();
        dc2.language = "fr".into();
        assert_ne!(e, hosted_etag(&r, &dc2, Some("h1"), false));
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
