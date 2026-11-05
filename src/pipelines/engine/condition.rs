//! Step and route conditions (`when = { ... }`).
//!
//! Every listed condition must hold. An empty condition always holds. A
//! failing condition yields a human-readable reason, recorded as the skip
//! reason of the step.

use serde::{Deserialize, Serialize};

use super::{ItemKind, RunContext};

/// Forge ids accepted by `forges`.
pub const FORGES: &[&str] = &["github", "gitlab", "gitea", "forgejo", "azure-devops"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// Only on issues, or only on PRs. On a step this is a scope: a step of
    /// the other item kind is left out of the run instead of being skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<ItemKind>,
    /// Repo toggles (`[repos.features]`) that must all be on, e.g.
    /// `["triage_issues"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Apply the repo's `[repos.features.filters]`: skip_authors, and
    /// triage_only/skip_labels + triage_max_age_days on issues, or
    /// target_branches on PRs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repo_filters: bool,
    /// The item carries at least one of these labels (case-insensitive).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels_any: Vec<String>,
    /// The item carries none of these labels (case-insensitive).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels_none: Vec<String>,
    /// The item author is one of these logins (case-insensitive).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    /// The item author is none of these logins (case-insensitive).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip_authors: Vec<String>,
    /// The repo is on one of these forges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forges: Vec<String>,
    /// PR only: at least one changed path matches one of these globs
    /// (`*` within a segment, `**` across segments, `?` one char).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// PR only: the analysis risk level (low, medium, high) is one of these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk: Vec<String>,
    /// The repo slug (`owner/name`) matches one of these globs. Mostly
    /// useful on routes of a shared (global) config.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,
}

impl Condition {
    pub fn is_empty(&self) -> bool {
        *self == Condition::default()
    }
}

/// Names of the boolean repo toggles, read from the serialized
/// `RepoFeatures` so a new toggle is usable here without code change.
pub fn known_features() -> Vec<String> {
    let v = serde_json::to_value(crate::config::RepoFeatures::default()).unwrap_or_default();
    let mut out: Vec<String> = v
        .as_object()
        .map(|o| {
            o.iter()
                .filter(|(_, v)| v.is_boolean())
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn feature_on(features: &crate::config::RepoFeatures, name: &str) -> bool {
    serde_json::to_value(features)
        .ok()
        .and_then(|v| v.get(name).and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

/// Glob → anchored regex (`**` any depth, `*` one segment, `?` one char).
pub fn glob_regex(glob: &str) -> Result<regex::Regex, String> {
    if glob.trim().is_empty() {
        return Err("empty glob".into());
    }
    let mut re = String::from("^");
    let chars: Vec<char> = glob.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    re.push_str("(?:.*/)?");
                    i += 3;
                } else {
                    re.push_str(".*");
                    i += 2;
                }
                continue;
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push('$');
    regex::Regex::new(&re).map_err(|e| format!("invalid glob '{glob}': {e}"))
}

fn glob_any(globs: &[String], text: &str) -> bool {
    globs
        .iter()
        .filter_map(|g| glob_regex(g).ok())
        .any(|re| re.is_match(text))
}

/// Static validation of a condition (feature names, forge ids, globs).
pub fn validate(c: &Condition) -> Result<(), String> {
    let known = known_features();
    for f in &c.features {
        if !known.contains(f) {
            return Err(format!(
                "unknown feature '{f}' in when.features (known: {})",
                known.join(", ")
            ));
        }
    }
    for f in &c.forges {
        if !FORGES.contains(&f.as_str()) {
            return Err(format!(
                "unknown forge '{f}' in when.forges (known: {})",
                FORGES.join(", ")
            ));
        }
    }
    for g in c.paths.iter().chain(&c.repos) {
        glob_regex(g)?;
    }
    if c.item == Some(ItemKind::Issue) && (!c.paths.is_empty() || !c.risk.is_empty()) {
        return Err("when.paths and when.risk only apply to PRs".into());
    }
    let empty = |v: &[String]| v.iter().any(|s| s.trim().is_empty());
    if empty(&c.labels_any)
        || empty(&c.labels_none)
        || empty(&c.authors)
        || empty(&c.skip_authors)
        || empty(&c.risk)
    {
        return Err("empty value in a when list".into());
    }
    Ok(())
}

fn has_label(labels: &[String], wanted: &str) -> bool {
    labels.iter().any(|l| l.eq_ignore_ascii_case(wanted))
}

/// Evaluate `c` for the run's item. `Ok(())` when every condition holds,
/// `Err(reason)` with the first one that does not.
pub async fn evaluate(c: &Condition, ctx: &RunContext) -> Result<(), String> {
    if let Some(item) = c.item {
        if item != ctx.item.kind {
            return Err(format!("only runs on {}", item.as_str()));
        }
    }
    if !c.repos.is_empty() && !glob_any(&c.repos, &ctx.config.repo_slug()) {
        return Err(format!("repo {} not in when.repos", ctx.config.repo_slug()));
    }
    if !c.forges.is_empty() {
        let forge = crate::git_provider::provider_id(&ctx.config);
        if !c.forges.iter().any(|f| f == forge) {
            return Err(format!("forge {forge} not in when.forges"));
        }
    }
    for f in &c.features {
        if !feature_on(&ctx.features, f) {
            return Err(format!("{f} disabled"));
        }
    }
    if c.repo_filters {
        repo_filters(ctx)?;
    }

    let needs_item = !c.labels_any.is_empty()
        || !c.labels_none.is_empty()
        || !c.authors.is_empty()
        || !c.skip_authors.is_empty();
    if needs_item {
        let info = ctx
            .item_info()
            .await
            .ok_or_else(|| format!("{} not found", ctx.item))?;
        if !c.labels_any.is_empty() && !c.labels_any.iter().any(|w| has_label(&info.labels, w)) {
            return Err(format!(
                "none of the labels {} on {}",
                c.labels_any.join(", "),
                ctx.item
            ));
        }
        if let Some(l) = c.labels_none.iter().find(|w| has_label(&info.labels, w)) {
            return Err(format!("label '{l}' on {}", ctx.item));
        }
        let author = info.author.as_deref().unwrap_or("");
        if !c.authors.is_empty() && !c.authors.iter().any(|a| a.eq_ignore_ascii_case(author)) {
            return Err(format!("author '{author}' not in when.authors"));
        }
        if c.skip_authors
            .iter()
            .any(|a| a.eq_ignore_ascii_case(author))
        {
            return Err(format!("author '{author}' in when.skip_authors"));
        }
    }

    if !c.risk.is_empty() || !c.paths.is_empty() {
        if ctx.item.kind != ItemKind::Pr {
            return Err("when.paths / when.risk only apply to PRs".into());
        }
        let n = ctx.item.number.ok_or("no PR number")?;
        if !c.risk.is_empty() {
            let risk = ctx
                .db
                .get_pr_analysis(n)
                .ok()
                .flatten()
                .map(|a| a.risk_level)
                .ok_or("no risk level yet (PR not analyzed)")?;
            if !c.risk.iter().any(|r| r.eq_ignore_ascii_case(&risk)) {
                return Err(format!("risk '{risk}' not in when.risk"));
            }
        }
        if !c.paths.is_empty() {
            let paths = ctx.changed_paths().await?;
            if !paths.iter().any(|p| glob_any(&c.paths, p)) {
                return Err("no changed path matches when.paths".into());
            }
        }
    }
    Ok(())
}

/// The repo filters as the event processor applied them before the engine:
/// read from the DB copy of the item (refreshed by the sync step), and only
/// when the item is cached.
fn repo_filters(ctx: &RunContext) -> Result<(), String> {
    let Some(n) = ctx.item.number else {
        return Ok(());
    };
    let f = &ctx.features.filters;
    match ctx.item.kind {
        ItemKind::Issue => {
            if let Ok(Some(issue)) = ctx.db.get_issue(n) {
                if f.is_author_skipped(issue.author.as_deref()) {
                    return Err(format!(
                        "author '{}' in skip_authors",
                        issue.author.as_deref().unwrap_or("?")
                    ));
                }
                if !f.issue_labels_pass_triage(&issue.labels) {
                    return Err("labels filter".into());
                }
                if !f.issue_age_ok(&issue.created_at) {
                    return Err(format!("older than {} days", f.triage_max_age_days));
                }
            }
        }
        ItemKind::Pr => {
            if let Ok(Some(pr)) = ctx.db.get_pull(n) {
                if f.is_author_skipped(pr.author.as_deref()) {
                    return Err(format!(
                        "author '{}' in skip_authors",
                        pr.author.as_deref().unwrap_or("?")
                    ));
                }
                if !f.branch_allowed(pr.base_ref.as_deref()) {
                    return Err(format!(
                        "base '{}' not in target_branches",
                        pr.base_ref.as_deref().unwrap_or("?")
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Changed file paths of a unified diff (`diff --git a/x b/y` headers; the
/// new path, or the old one for a deletion).
pub fn diff_paths(diff: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some((a, b)) = rest.split_once(" b/") {
                let b = b.trim();
                let a = a.trim().strip_prefix("a/").unwrap_or(a.trim());
                let p = if b == "/dev/null" || b.is_empty() {
                    a
                } else {
                    b
                };
                if !out.iter().any(|x: &String| x == p) {
                    out.push(p.to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipelines::engine::tests_support::{ctx_with, seed_issue, seed_pull};

    #[test]
    fn globs() {
        let m = |g: &str, t: &str| glob_regex(g).unwrap().is_match(t);
        assert!(m("docs/**", "docs/a/b.md"));
        assert!(m("**/*.md", "README.md"));
        assert!(m("**/*.md", "a/b/c.md"));
        assert!(m("*.md", "README.md"));
        assert!(!m("*.md", "docs/README.md"));
        assert!(m("src/?.rs", "src/a.rs"));
        assert!(!m("src/?.rs", "src/ab.rs"));
        assert!(m("wshm-dev/*", "wshm-dev/wshm"));
        assert!(!m("a.b", "axb"), "dots are literal");
        assert!(glob_regex("").is_err());
    }

    #[test]
    fn diff_paths_parsing() {
        let diff = "diff --git a/src/x.rs b/src/x.rs\n+a\ndiff --git a/old.md b/new.md\n";
        assert_eq!(diff_paths(diff), ["src/x.rs", "new.md"]);
    }

    #[test]
    fn known_features_lists_toggles() {
        let k = known_features();
        for f in [
            "collect_issues",
            "collect_prs",
            "triage_issues",
            "analyze_prs",
            "review_prs",
            "auto_pr",
            "auto_merge",
        ] {
            assert!(k.contains(&f.to_string()), "{f}");
        }
        assert!(!k.contains(&"filters".to_string()));
    }

    #[tokio::test]
    async fn evaluate_features_item_and_labels() {
        let features = crate::config::RepoFeatures {
            triage_issues: true,
            ..Default::default()
        };
        let ctx = ctx_with(ItemKind::Issue, Some(7), features);
        seed_issue(&ctx, 7, &["Bug", "area/x"], "alice");

        let ok = |c: Condition| {
            let ctx = &ctx;
            async move { evaluate(&c, ctx).await }
        };
        assert!(ok(Condition::default()).await.is_ok());
        assert!(ok(Condition {
            features: vec!["triage_issues".into()],
            ..Default::default()
        })
        .await
        .is_ok());
        assert_eq!(
            ok(Condition {
                features: vec!["analyze_prs".into()],
                ..Default::default()
            })
            .await
            .unwrap_err(),
            "analyze_prs disabled"
        );
        assert!(ok(Condition {
            item: Some(ItemKind::Pr),
            ..Default::default()
        })
        .await
        .is_err());
        assert!(ok(Condition {
            labels_any: vec!["bug".into()],
            ..Default::default()
        })
        .await
        .is_ok());
        assert!(ok(Condition {
            labels_none: vec!["BUG".into()],
            ..Default::default()
        })
        .await
        .is_err());
        assert!(ok(Condition {
            authors: vec!["bob".into()],
            ..Default::default()
        })
        .await
        .is_err());
        assert!(ok(Condition {
            skip_authors: vec!["Alice".into()],
            ..Default::default()
        })
        .await
        .is_err());
        assert!(ok(Condition {
            forges: vec!["github".into()],
            ..Default::default()
        })
        .await
        .is_ok());
        assert!(ok(Condition {
            forges: vec!["gitlab".into()],
            ..Default::default()
        })
        .await
        .is_err());
        assert!(ok(Condition {
            risk: vec!["high".into()],
            ..Default::default()
        })
        .await
        .is_err());
    }

    #[tokio::test]
    async fn repo_filters_match_legacy_gates() {
        let mut features = crate::config::RepoFeatures::default();
        features.filters.skip_authors = vec!["bot".into()];
        features.filters.target_branches = vec!["main".into()];
        let c = Condition {
            repo_filters: true,
            ..Default::default()
        };

        let ctx = ctx_with(ItemKind::Issue, Some(1), features.clone());
        // Not cached: passes (the legacy gate only ran on a cached item).
        assert!(evaluate(&c, &ctx).await.is_ok());
        seed_issue(&ctx, 1, &[], "bot");
        assert!(evaluate(&c, &ctx)
            .await
            .unwrap_err()
            .contains("skip_authors"));

        let ctx = ctx_with(ItemKind::Pr, Some(2), features);
        seed_pull(&ctx, 2, "human", "dev");
        assert!(evaluate(&c, &ctx)
            .await
            .unwrap_err()
            .contains("target_branches"));
        seed_pull(&ctx, 2, "human", "main");
        assert!(evaluate(&c, &ctx).await.is_ok());
    }

    #[tokio::test]
    async fn risk_condition_reads_analysis() {
        let ctx = ctx_with(ItemKind::Pr, Some(3), Default::default());
        seed_pull(&ctx, 3, "a", "main");
        let c = Condition {
            risk: vec!["high".into()],
            ..Default::default()
        };
        assert!(evaluate(&c, &ctx)
            .await
            .unwrap_err()
            .contains("not analyzed"));
        ctx.db
            .upsert_pr_analysis(&crate::db::pulls::PrAnalysisRow {
                pr_number: 3,
                summary: "s".into(),
                risk_level: "high".into(),
                pr_type: "feature".into(),
                review_notes: None,
                domains: Vec::new(),
                analyzed_at: chrono::Utc::now().to_rfc3339(),
                content_hash: None,
            })
            .unwrap();
        assert!(evaluate(&c, &ctx).await.is_ok());
    }

    #[test]
    fn validate_rejects_bad_values() {
        assert!(validate(&Condition {
            forges: vec!["svn".into()],
            ..Default::default()
        })
        .is_err());
        assert!(validate(&Condition {
            item: Some(ItemKind::Issue),
            paths: vec!["a".into()],
            ..Default::default()
        })
        .is_err());
        assert!(validate(&Condition {
            labels_any: vec![" ".into()],
            ..Default::default()
        })
        .is_err());
    }
}
