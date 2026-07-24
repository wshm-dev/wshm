//! Domain discovery: infer a repo's "grand domains" from cheap signals so the
//! user doesn't have to guess them. Merges the result into the DB-backed
//! `review_domains` set as PROPOSED (validated = false) — a human validates
//! them in Settings before they become `domain:*` GitHub labels.
//!
//! Signals: repo languages + top-level structure + root manifests (GitHub API)
//! and recent PR / issue titles (already synced in the DB). No clone needed —
//! works on stateless pods.

use anyhow::Result;

use crate::ai::backend::AiBackend;
use crate::ai::prompts::discover_domains;
use crate::ai::schemas::DomainDiscovery;
use crate::config::{Config, DomainDef};
use crate::db::backend::DatabaseBackend;
use crate::db::settings::REVIEW_DOMAINS_KEY;
use crate::github::Client as GhClient;

/// Marker so auto-discovery runs at most once per repo (the button forces a
/// fresh run regardless). Stored alongside the domains in `app_settings`.
pub const DOMAINS_DISCOVERED_KEY: &str = "domains_discovered";

fn load_domains(db: &dyn DatabaseBackend) -> Vec<DomainDef> {
    db.get_app_setting(REVIEW_DOMAINS_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Run discovery and merge newly-found domains (as proposed) into the DB set.
/// Existing domains keep their `validated` flag. Returns the full merged set.
pub async fn discover(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    ai: &AiBackend,
) -> Result<Vec<DomainDef>> {
    let languages = gh.fetch_languages().await.unwrap_or_default();
    let entries = gh.fetch_root_entries().await.unwrap_or_default();
    let pr_titles: Vec<String> = db
        .get_open_pulls()
        .unwrap_or_default()
        .iter()
        .take(40)
        .map(|p| p.title.clone())
        .collect();
    let issue_titles: Vec<String> = db
        .get_open_issues()
        .unwrap_or_default()
        .iter()
        .take(40)
        .map(|i| i.title.clone())
        .collect();

    let user = discover_domains::build_user_prompt(&languages, &entries, &pr_titles, &issue_titles);
    let out: DomainDiscovery = ai.complete(discover_domains::SYSTEM, &user).await?;

    let mut existing = load_domains(db);
    for d in out.domains {
        let name = d.name.trim().to_lowercase();
        if name.is_empty() {
            continue;
        }
        if !existing.iter().any(|e| e.name.eq_ignore_ascii_case(&name)) {
            existing.push(DomainDef {
                name,
                description: d.description,
                validated: false,
            });
        }
    }
    if let Ok(j) = serde_json::to_string(&existing) {
        let _ = db.set_app_setting(REVIEW_DOMAINS_KEY, &j);
    }
    let _ = db.set_app_setting(DOMAINS_DISCOVERED_KEY, "1");
    tracing::info!(
        "Domain discovery for {}: {} domains total",
        config.repo_slug(),
        existing.len()
    );
    Ok(existing)
}

/// Run discovery only once per repo (auto path): if no domains are configured
/// AND discovery hasn't been attempted yet. Best-effort — logs and swallows
/// errors so it never blocks the review batch.
pub async fn ensure_discovered(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    ai: &AiBackend,
) {
    let has_domains = !load_domains(db).is_empty();
    let attempted = db
        .get_app_setting(DOMAINS_DISCOVERED_KEY)
        .ok()
        .flatten()
        .is_some();
    if has_domains || attempted {
        return;
    }
    if let Err(e) = discover(config, db, gh, ai).await {
        tracing::warn!("Auto domain discovery failed: {e}");
        // Mark attempted so we don't retry every batch on a persistent failure.
        let _ = db.set_app_setting(DOMAINS_DISCOVERED_KEY, "1");
    }
}
