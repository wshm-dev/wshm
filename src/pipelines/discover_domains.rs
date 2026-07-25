//! Domain discovery: infer a repo's "grand domains" from what its PRs and
//! issues are actually about. Ranks the whole title corpus by subject frequency
//! and merges the top subjects into the DB-backed `review_domains` set as
//! PROPOSED (validated = false) — a human validates them in Settings before
//! they become `domain:*` GitHub labels.
//!
//! Titles only (already synced in the DB): deterministic, instant, no AI call,
//! no clone — works on stateless pods.

use anyhow::Result;

use crate::config::{Config, DomainDef};
use crate::db::backend::DatabaseBackend;
use crate::db::settings::REVIEW_DOMAINS_KEY;

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

/// Frequency-rank the significant words across all PR + issue titles, dropping
/// stopwords and generic dev-process verbs (fix/add/update/…) that dominate
/// commit-style titles but say nothing about a repo's *domains*. Tokens keep
/// `#`/`+` (so "c#", "c++" survive) but anything containing a digit or a `#`
/// prefix (issue refs, versions) is dropped. Returns the top `n` terms seen at
/// least twice, most frequent first.
fn top_terms(pr_titles: &[String], issue_titles: &[String], n: usize) -> Vec<(String, usize)> {
    use std::collections::{HashMap, HashSet};
    const STOP: &[&str] = &[
        // articles / glue
        "the",
        "a",
        "an",
        "to",
        "of",
        "for",
        "in",
        "on",
        "and",
        "or",
        "with",
        "without",
        "by",
        "is",
        "are",
        "be",
        "this",
        "that",
        "it",
        "its",
        "as",
        "at",
        "from",
        "into",
        "via",
        "when",
        "not",
        "no",
        "up",
        "out",
        "off",
        "if",
        "then",
        "else",
        "per",
        "vs",
        "so",
        // generic dev-process verbs/nouns — noise, not domains
        "fix",
        "fixes",
        "fixed",
        "add",
        "adds",
        "added",
        "adding",
        "update",
        "updates",
        "updated",
        "bump",
        "chore",
        "feat",
        "feature",
        "features",
        "refactor",
        "remove",
        "removes",
        "removed",
        "test",
        "tests",
        "testing",
        "wip",
        "merge",
        "release",
        "docs",
        "doc",
        "ci",
        "cd",
        "build",
        "support",
        "improve",
        "improvement",
        "improvements",
        "use",
        "using",
        "used",
        "make",
        "makes",
        "allow",
        "ensure",
        "handle",
        "new",
        "better",
        "error",
        "errors",
        "issue",
        "issues",
        "pr",
        "prs",
        "bug",
        "bugs",
        "wrong",
        "broken",
        "fail",
        "fails",
        "failing",
        "cleanup",
        "clean",
        "rework",
        "change",
        "changes",
        "changed",
        "revert",
        "wshm",
        "pro",
    ];
    let stop: HashSet<&str> = STOP.iter().copied().collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for t in pr_titles.iter().chain(issue_titles.iter()) {
        for raw in t.split(|c: char| !c.is_alphanumeric() && c != '#' && c != '+') {
            let w = raw.to_lowercase();
            if w.len() < 2
                || w.starts_with('#')
                || w.chars().any(|c| c.is_ascii_digit())
                || stop.contains(w.as_str())
            {
                continue;
            }
            *counts.entry(w).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().filter(|(_, c)| *c >= 2).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(n);
    ranked
}

/// Split a title into lowercased word tokens (keeping `#`/`+` so "c#"/"c++"
/// survive). Shared by the ranker and the domain counter so both see terms the
/// same way.
fn title_words(title: &str) -> std::collections::HashSet<String> {
    title
        .split(|c: char| !c.is_alphanumeric() && c != '#' && c != '+')
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 2)
        .collect()
}

/// Effective "all PRs" cap for the corpus — high enough to cover every closed
/// PR in practice (title-only, so even tens of thousands is cheap) while still
/// bounding memory on pathological repos.
const CORPUS_CLOSED_CAP: usize = 100_000;

/// The whole title corpus (open + ALL closed PRs + open issues) as titles.
fn corpus_titles(db: &dyn DatabaseBackend) -> Vec<String> {
    let mut titles: Vec<String> = db
        .get_open_pulls()
        .unwrap_or_default()
        .iter()
        .map(|p| p.title.clone())
        .collect();
    titles.extend(
        db.get_closed_pulls(CORPUS_CLOSED_CAP)
            .unwrap_or_default()
            .iter()
            .map(|p| p.title.clone()),
    );
    titles.extend(
        db.get_open_issues()
            .unwrap_or_default()
            .iter()
            .map(|i| i.title.clone()),
    );
    titles
}

/// How many PRs/issues each domain groups: the count of corpus titles that
/// mention it — matching the domain's slug OR any of its hyphen/underscore
/// parts as a whole word (so "web-ui" matches titles with "web" or "ui").
/// This is the "25 PRs on codex" volume shown next to each domain; computed
/// live (never persisted) so it stays fresh as PRs come and go.
pub fn domain_counts(
    db: &dyn DatabaseBackend,
    domains: &[DomainDef],
) -> std::collections::HashMap<String, usize> {
    let title_sets: Vec<std::collections::HashSet<String>> =
        corpus_titles(db).iter().map(|t| title_words(t)).collect();
    let mut out = std::collections::HashMap::new();
    for d in domains {
        let name = d.name.to_lowercase();
        // The slug itself plus its parts (drop 1-char fragments).
        let mut toks: Vec<String> = name
            .split(['-', '_', ' '])
            .filter(|s| s.len() >= 2)
            .map(|s| s.to_string())
            .collect();
        toks.push(name.clone());
        let count = title_sets
            .iter()
            .filter(|set| toks.iter().any(|tk| set.contains(tk)))
            .count();
        out.insert(d.name.clone(), count);
    }
    out
}

/// How many domains discovery proposes: the top subjects by PR/issue volume.
const MAX_DISCOVERED: usize = 12;

/// Discover a repo's grand domains from the *frequency of subjects across ALL
/// its PRs and issues* — the terms the most pull requests are actually about
/// (git, codex, windows…). Deterministic and instant: no AI call, no clone, no
/// network — it ranks the whole title corpus, takes the top subjects, and
/// merges them into the DB set as PROPOSED.
///
/// Being AI-free is deliberate: the old AI pass took ~40s and timed out behind
/// the auth proxy (browser saw an HTML 504, not JSON). Human-validated domains
/// are preserved; only proposals are regenerated, so re-running never piles up
/// duplicates. Returns the full merged set.
pub async fn discover(config: &Config, db: &dyn DatabaseBackend) -> Result<Vec<DomainDef>> {
    // Whole corpus: open + ALL closed PRs + open issues (titles only).
    let titles = corpus_titles(db);
    let ranked = top_terms(&titles, &[], MAX_DISCOVERED);

    // Keep human-validated domains; REPLACE the proposed set with this fresh run
    // so re-discovering dedupes instead of accumulating near-duplicates.
    let mut existing: Vec<DomainDef> = load_domains(db)
        .into_iter()
        .filter(|d| d.validated)
        .collect();
    for (term, _count) in ranked {
        if !existing.iter().any(|e| e.name.eq_ignore_ascii_case(&term)) {
            existing.push(DomainDef {
                name: term,
                description: None,
                validated: false,
            });
        }
    }
    if let Ok(j) = serde_json::to_string(&existing) {
        let _ = db.set_app_setting(REVIEW_DOMAINS_KEY, &j);
    }
    let _ = db.set_app_setting(DOMAINS_DISCOVERED_KEY, "1");
    tracing::info!(
        "Domain discovery for {}: {} domains total (from {} PR/issue titles)",
        config.repo_slug(),
        existing.len(),
        titles.len()
    );
    Ok(existing)
}

/// Run discovery only once per repo (auto path): if no domains are configured
/// AND discovery hasn't been attempted yet. Best-effort — logs and swallows
/// errors so it never blocks the review batch.
pub async fn ensure_discovered(config: &Config, db: &dyn DatabaseBackend) {
    let has_domains = !load_domains(db).is_empty();
    let attempted = db
        .get_app_setting(DOMAINS_DISCOVERED_KEY)
        .ok()
        .flatten()
        .is_some();
    if has_domains || attempted {
        return;
    }
    if let Err(e) = discover(config, db).await {
        tracing::warn!("Auto domain discovery failed: {e}");
        // Mark attempted so we don't retry every batch on a persistent failure.
        let _ = db.set_app_setting(DOMAINS_DISCOVERED_KEY, "1");
    }
}

#[cfg(test)]
mod tests {
    use super::top_terms;

    #[test]
    fn ranks_domain_terms_and_drops_noise() {
        let prs = vec![
            "Fix codex parser crash".to_string(),
            "feat(codex): add streaming".to_string(),
            "Improve codex prompts".to_string(),
            "bun: bump runtime".to_string(),
            "Update bun lockfile".to_string(),
            "C# analyzer for billing".to_string(),
            "billing webhook #123 retry".to_string(),
        ];
        let issues = vec![
            "codex times out".to_string(),
            "billing invoice wrong total".to_string(),
        ];
        let terms = top_terms(&prs, &issues, 10);
        let map: std::collections::HashMap<_, _> = terms.iter().cloned().collect();

        // Recurrent domain words are counted…
        assert_eq!(map.get("codex"), Some(&4));
        assert_eq!(map.get("billing"), Some(&3));
        assert_eq!(map.get("bun"), Some(&2));
        // …process verbs and singletons/refs/versions are dropped.
        assert!(!map.contains_key("fix"));
        assert!(!map.contains_key("add"));
        assert!(!map.contains_key("update"));
        assert!(!map.contains_key("#123"));
        assert!(!map.contains_key("parser")); // appears once -> below the >=2 floor
                                              // Ranking is frequency-desc: codex first.
        assert_eq!(terms.first().map(|(t, _)| t.as_str()), Some("codex"));
    }

    #[test]
    fn keeps_hashy_tech_tokens() {
        // "c#" recurs -> survives tokenization (digit-free, no '#' prefix).
        let prs = vec!["C# refactor".to_string(), "port to c# core".to_string()];
        let terms = top_terms(&prs, &[], 10);
        assert!(terms.iter().any(|(t, c)| t == "c#" && *c == 2));
    }
}
