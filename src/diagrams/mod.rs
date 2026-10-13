//! PR diagrams (`[pr.review_diagrams]`): an architecture graph of the PR,
//! drawn as Mermaid in the PR analysis comment and optionally published as
//! an interactive page.
//!
//! Flow: one AI call (after per-file summaries for large diffs) returns a
//! graph JSON, validated by [`graph::normalize`], cached per PR in
//! `pr_diagrams` and regenerated only when the diff or the generation
//! settings change. Everything is fail-soft: an error means no diagram, the
//! rest of the comment is unchanged.

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

    let page_ok = previous
        .as_ref()
        .filter(|p| !fresh && p.page_target.as_deref() == Some(target) && p.page_key.is_some());
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
        row.page_html = p.page_html.clone();
        row.is_private = p.is_private;
    } else if dc.publish.target != DiagramPublishTarget::Off {
        let is_private = match gh.fetch_repo_private().await {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    "Could not read the visibility of {}, assuming private: {e:#}",
                    config.repo_slug()
                );
                true
            }
        };
        let html = page::render(
            &graph,
            &page::PageOptions {
                language: &dc.language,
                repo: &config.repo_slug(),
                pr: pr.number,
                diff: dc.diff_viewer.then_some(diff),
            },
        );
        match publish::publish(
            &dc.publish,
            gh,
            &config.repo_slug(),
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
                row.page_html = p.html;
                row.is_private = is_private;
            }
            Ok(None) => {}
            Err(e) => warn!("PR #{}: diagram page not published: {e:#}", pr.number),
        }
    }
    db.upsert_pr_diagram(&row)?;

    Ok(render::comment_section(
        &graph,
        &render::SectionOptions {
            language: &dc.language,
            text: dc.text,
            comment: dc.comment,
            page_url: row.page_url.as_deref(),
            page_has_diff: dc.diff_viewer,
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
/// of PRs no longer open, and pages older than `retention_days`.
pub async fn sweep(config: &Config, db: &dyn DatabaseBackend, gh: &GhClient) {
    let Ok(rows) = db.list_pr_diagrams() else {
        return;
    };
    if rows.is_empty() {
        return;
    }
    let dc = crate::config::load_review_diagrams(config, db);
    let cutoff = (dc.publish.retention_days > 0)
        .then(|| chrono::Utc::now() - chrono::Duration::days(dc.publish.retention_days as i64));
    for row in rows {
        let open = matches!(db.get_pull(row.pr_number), Ok(Some(p)) if p.state == "open");
        if !open {
            cleanup_pr(config, db, gh, row.pr_number).await;
            continue;
        }
        let expired = cutoff.is_some_and(|c| {
            chrono::DateTime::parse_from_rfc3339(&row.updated_at)
                .map(|t| t < c)
                .unwrap_or(false)
        });
        if expired && row.page_target.is_some() {
            if let Err(e) = publish::delete_page(&dc.publish, gh, &row).await {
                warn!(
                    "PR #{}: expired diagram page cleanup failed: {e:#}",
                    row.pr_number
                );
                continue;
            }
            // Keep the graph cache, forget the page (re-published on next push).
            if let Ok(Some(mut full)) = db.get_pr_diagram(row.pr_number) {
                full.page_target = None;
                full.page_key = None;
                full.page_url = None;
                full.page_token = None;
                full.page_html = None;
                let _ = db.upsert_pr_diagram(&full);
            }
        }
    }
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
}
