//! Validation of the model's output before wshm acts on it or posts it
//! (#235). The model only proposes: everything it returns is checked against
//! allowlists, and anything outside them is dropped.
//!
//! - Labels must be in the configured label set, or (when none is
//!   configured) in the standard set or already used in the repository.
//! - Categories, priorities, risk levels, PR types, severities and review
//!   categories must be in their fixed sets.
//! - A duplicate must point at a known issue; linked issues must be
//!   referenced in the PR itself; inline review comments must target a line
//!   that exists on the new side of the diff.
//! - Text that wshm posts (summaries, actions, review comments, suggested
//!   issues) loses its mentions, links and HTML, see [`sanitize_posted_text`].

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::ai::schemas::{InlineComment, InlineReviewResult, IssueClassification, PrAnalysis};

// ── Posted text ─────────────────────────────────────────────────────

static RE_HTML_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?(?:-->|$)").unwrap());
static RE_HTML_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"</?[A-Za-z!][^<>]*>").unwrap());
static RE_MD_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[([^\]\n]*)\]\([^)\n]*\)").unwrap());
static RE_MD_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\n]*)\]\([^)\n]*\)").unwrap());
static RE_MD_REFDEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]{0,3}\[[^\]\n]+\]:[^\n]*$").unwrap());
static RE_MD_REFLINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]\n]+)\]\[[^\]\n]*\]").unwrap());
static RE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:https?|ftp|file|ssh|git)://\S+|\bwww\.[a-z0-9-]+\.\S+").unwrap()
});
static RE_SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:javascript|vbscript|data):").unwrap());
static RE_MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^A-Za-z0-9_`])@([A-Za-z0-9][A-Za-z0-9_/.-]*)").unwrap());

/// Neutralise `@user` / `@org/team` mentions (a zero-width space after the
/// `@` keeps the text readable but does not notify anyone).
pub fn neutralize_mentions(s: &str) -> String {
    RE_MENTION.replace_all(s, "$1@\u{200B}$2").into_owned()
}

/// Make AI-written text safe to post on the forge: invisible characters,
/// HTML (tags and comments), links (Markdown, reference-style, bare URLs,
/// dangerous schemes) and mentions are removed or neutralised, then the
/// text is capped at `max_chars` characters.
pub fn sanitize_posted_text(s: &str, max_chars: usize) -> String {
    let mut out = crate::ai::untrusted::normalize(s, crate::ai::untrusted::ContentKind::Code);
    // `normalize` prefixes notes when it removed something: not wanted in
    // posted text.
    out = out
        .lines()
        .filter(|l| !l.starts_with("[wshm: "))
        .collect::<Vec<_>>()
        .join("\n");
    out = RE_HTML_COMMENT.replace_all(&out, "").into_owned();
    out = RE_HTML_TAG.replace_all(&out, "").into_owned();
    out = RE_MD_IMAGE.replace_all(&out, "$1").into_owned();
    out = RE_MD_LINK.replace_all(&out, "$1").into_owned();
    out = RE_MD_REFDEF.replace_all(&out, "").into_owned();
    out = RE_MD_REFLINK.replace_all(&out, "$1").into_owned();
    out = RE_URL.replace_all(&out, "[link removed]").into_owned();
    out = RE_SCHEME.replace_all(&out, "").into_owned();
    out = neutralize_mentions(&out);
    let out = out.trim();
    if out.chars().count() <= max_chars {
        out.to_string()
    } else {
        let mut t: String = out.chars().take(max_chars).collect();
        t.push('…');
        t
    }
}

/// A repository path as the model reported it, kept only when it looks like
/// a plain relative path (it is posted inside backticks).
pub fn sanitize_path(p: &str) -> Option<String> {
    let p = p.trim().trim_start_matches("./");
    let ok = !p.is_empty()
        && p.len() <= 200
        && !p.starts_with('/')
        && !p.split('/').any(|seg| seg == "..")
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+' | '@'));
    ok.then(|| p.to_string())
}

/// A lowercase slug (domain names): `[a-z0-9][a-z0-9-]{0,39}`.
pub fn is_slug(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

// ── Labels ──────────────────────────────────────────────────────────

/// Labels listed in the default triage prompt. Used as the allowlist (with
/// the labels already used in the repository) when no `[[labels]]` are
/// configured.
pub const STANDARD_LABELS: &[&str] = &[
    "bug",
    "enhancement",
    "documentation",
    "question",
    "duplicate",
    "wontfix",
    "invalid",
    "priority:critical",
    "priority:high",
    "priority:medium",
    "priority:low",
    "good first issue",
    "help wanted",
    "needs-triage",
    "needs-info",
    "needs-reproduction",
    "area:cli",
    "area:config",
    "area:api",
    "area:ui",
    "area:ci",
    "area:security",
    "area:performance",
    "area:testing",
    "area:docs",
    "platform:windows",
    "platform:linux",
    "platform:macos",
];

/// Which labels the model may apply.
#[derive(Debug, Clone, Default)]
pub struct LabelPolicy {
    /// Lowercased allowed names.
    allowed: HashSet<String>,
    /// Lowercased names the model may never apply (approval label, markers).
    forbidden: HashSet<String>,
}

impl LabelPolicy {
    /// `configured`: the `[[labels]]` names (exclusive when non-empty).
    /// `existing`: labels already used in the repository.
    pub fn new<'a>(
        configured: impl IntoIterator<Item = &'a str>,
        existing: impl IntoIterator<Item = &'a str>,
        forbidden: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let configured: HashSet<String> = configured.into_iter().map(str::to_lowercase).collect();
        let allowed = if configured.is_empty() {
            STANDARD_LABELS
                .iter()
                .copied()
                .chain(existing)
                .map(str::to_lowercase)
                .collect()
        } else {
            configured
        };
        Self {
            allowed,
            forbidden: forbidden
                .into_iter()
                .map(str::to_lowercase)
                .filter(|s| !s.is_empty())
                .collect(),
        }
    }

    pub fn allows(&self, label: &str) -> bool {
        let l = label.trim().to_lowercase();
        !l.is_empty()
            && !self.forbidden.contains(&l)
            // Domain labels are applied by wshm only for validated domains;
            // `wshm:` labels are control markers set by maintainers.
            && !l.starts_with("domain:")
            && !l.starts_with("wshm:")
            && self.allowed.contains(&l)
    }

    /// Keep allowed labels (deduplicated, original spelling of the first
    /// occurrence); returns the dropped ones too.
    pub fn filter(&self, labels: Vec<String>) -> (Vec<String>, Vec<String>) {
        let mut seen = HashSet::new();
        let (mut kept, mut dropped) = (Vec::new(), Vec::new());
        for l in labels {
            let t = l.trim().to_string();
            if self.allows(&t) {
                if seen.insert(t.to_lowercase()) {
                    kept.push(t);
                }
            } else {
                dropped.push(l);
            }
        }
        (kept, dropped)
    }
}

/// Every label name used on the given label lists (issues/PRs from the DB).
pub fn existing_labels<'a>(lists: impl IntoIterator<Item = &'a Vec<String>>) -> HashSet<String> {
    lists.into_iter().flatten().map(|l| l.to_string()).collect()
}

// ── Issue classification ────────────────────────────────────────────

pub const CATEGORIES: &[&str] = &[
    "bug",
    "feature",
    "duplicate",
    "wontfix",
    "needs-info",
    "question",
    "docs",
];
pub const PRIORITIES: &[&str] = &["critical", "high", "medium", "low"];

/// What [`validate_classification`] needs to know about the repository.
pub struct ClassificationContext<'a> {
    pub issue_number: u64,
    /// Issue numbers known to exist (duplicate targets).
    pub known_issues: &'a HashSet<u64>,
    pub labels: &'a LabelPolicy,
}

/// Bring a classification back inside the allowed sets. Returns a short
/// description of each correction (for the logs).
pub fn validate_classification(
    c: &mut IssueClassification,
    ctx: &ClassificationContext,
) -> Vec<String> {
    let mut fixes = Vec::new();
    let cat = c.category.trim().to_lowercase();
    if CATEGORIES.contains(&cat.as_str()) {
        c.category = cat;
    } else {
        fixes.push(format!("unknown category {:?} -> needs-info", c.category));
        c.category = "needs-info".to_string();
    }
    if let Some(p) = c.priority.take() {
        let p2 = p.trim().to_lowercase();
        let p2 = p2.strip_prefix("priority:").unwrap_or(&p2).to_string();
        if PRIORITIES.contains(&p2.as_str()) {
            c.priority = Some(p2);
        } else {
            fixes.push(format!("unknown priority {p:?} dropped"));
        }
    }
    if let Some(d) = c.is_duplicate_of {
        if d == ctx.issue_number || !ctx.known_issues.contains(&d) {
            fixes.push(format!("duplicate target #{d} is not a known issue"));
            c.is_duplicate_of = None;
        }
    }
    let (kept, dropped) = ctx.labels.filter(std::mem::take(&mut c.suggested_labels));
    if !dropped.is_empty() {
        fixes.push(format!("labels outside the allowlist dropped: {dropped:?}"));
    }
    c.suggested_labels = kept;
    c.summary = sanitize_posted_text(&c.summary, 500);
    c.suggested_actions = std::mem::take(&mut c.suggested_actions)
        .into_iter()
        .map(|a| sanitize_posted_text(&a, 120))
        .filter(|a| !a.is_empty())
        .take(4)
        .collect();
    let before = c.relevant_files.len();
    c.relevant_files = std::mem::take(&mut c.relevant_files)
        .iter()
        .filter_map(|p| sanitize_path(p))
        .take(10)
        .collect();
    if c.relevant_files.len() < before.min(10) {
        fixes.push("invalid relevant file paths dropped".to_string());
    }
    validate_domains(&mut c.domains, &mut fixes);
    fixes
}

fn validate_domains(domains: &mut Vec<String>, fixes: &mut Vec<String>) {
    let before = domains.len();
    let mut seen = HashSet::new();
    *domains = std::mem::take(domains)
        .into_iter()
        .map(|d| d.trim().to_lowercase())
        .filter(|d| is_slug(d) && seen.insert(d.clone()))
        .take(5)
        .collect();
    if domains.len() < before {
        fixes.push("invalid or extra domains dropped".to_string());
    }
}

// ── PR analysis ─────────────────────────────────────────────────────

pub const RISK_LEVELS: &[&str] = &["low", "medium", "high"];
pub const PR_TYPES: &[&str] = &["bug-fix", "feature", "refactor", "docs", "chore"];

static RE_ISSUE_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#(\d{1,9})\b").unwrap());

/// Issue numbers written as `#N` in `text`.
pub fn referenced_numbers(text: &str) -> HashSet<u64> {
    RE_ISSUE_REF
        .captures_iter(text)
        .filter_map(|c| c[1].parse().ok())
        .collect()
}

/// Bring a PR analysis back inside the allowed sets. An unknown risk level
/// becomes "high" (the conservative reading). Linked issues must be
/// referenced in the PR title or body.
pub fn validate_pr_analysis(
    a: &mut PrAnalysis,
    pr_title: &str,
    pr_body: &str,
    labels: &LabelPolicy,
) -> Vec<String> {
    let mut fixes = Vec::new();
    let risk = a.risk_level.trim().to_lowercase();
    if RISK_LEVELS.contains(&risk.as_str()) {
        a.risk_level = risk;
    } else {
        fixes.push(format!("unknown risk level {:?} -> high", a.risk_level));
        a.risk_level = "high".to_string();
    }
    let t = a.pr_type.trim().to_lowercase();
    if PR_TYPES.contains(&t.as_str()) {
        a.pr_type = t;
    } else {
        fixes.push(format!("unknown PR type {:?} -> chore", a.pr_type));
        a.pr_type = "chore".to_string();
    }
    let refs = referenced_numbers(&format!("{pr_title}\n{pr_body}"));
    let before = a.linked_issues.len();
    let mut seen = HashSet::new();
    a.linked_issues
        .retain(|n| refs.contains(n) && seen.insert(*n));
    if a.linked_issues.len() < before {
        fixes.push("linked issues not referenced in the PR dropped".to_string());
    }
    let (kept, dropped) = labels.filter(std::mem::take(&mut a.suggested_labels));
    if !dropped.is_empty() {
        fixes.push(format!("labels outside the allowlist dropped: {dropped:?}"));
    }
    a.suggested_labels = kept;
    a.summary = sanitize_posted_text(&a.summary, 500);
    validate_domains(&mut a.domains, &mut fixes);
    fixes
}

// ── Inline review ───────────────────────────────────────────────────

pub const SEVERITIES: &[&str] = &["error", "warning", "info"];
pub const REVIEW_CATEGORIES: &[&str] = &[
    "bug",
    "security",
    "perf",
    "race-condition",
    "resource-leak",
    "logic",
    "error-handling",
];

/// Lines that exist on the new side of a unified diff, per file path
/// (added and context lines: the ones an inline comment can target).
pub fn new_side_lines(diff: &str) -> HashMap<String, HashSet<u64>> {
    static RE_HUNK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@").unwrap());
    let mut out: HashMap<String, HashSet<u64>> = HashMap::new();
    let mut path: Option<String> = None;
    let mut line: Option<u64> = None;
    for l in diff.lines() {
        if l.starts_with("diff --git ") {
            path = l.rsplit(" b/").next().map(str::to_string);
            line = None;
        } else if let (Some(p), None) = (l.strip_prefix("+++ "), line) {
            path = match p.strip_prefix("b/") {
                Some(p) => Some(p.to_string()),
                None if p == "/dev/null" => None,
                None => Some(p.to_string()),
            };
            line = None;
        } else if l.starts_with("--- ") && line.is_none() {
            // old-side header
        } else if let Some(c) = RE_HUNK.captures(l) {
            line = c[1].parse().ok();
        } else if let (Some(p), Some(n)) = (path.as_ref(), line) {
            if l.starts_with('+') || l.starts_with(' ') || l.is_empty() {
                out.entry(p.clone()).or_default().insert(n);
                line = Some(n + 1);
            } else if l.starts_with('-') || l.starts_with('\\') {
                // removed line / "\ No newline at end of file"
            }
        }
    }
    out
}

/// Keep the inline comments that target a real new-side line of `diff`,
/// with allowed severity/category, sanitized body and a suggestion that
/// cannot break out of its code fence. Returns the number dropped.
pub fn validate_review(r: &mut InlineReviewResult, diff: &str) -> usize {
    let lines = new_side_lines(diff);
    let before = r.comments.len();
    r.comments = std::mem::take(&mut r.comments)
        .into_iter()
        .filter_map(|c| validate_inline_comment(c, &lines))
        .collect();
    r.summary = sanitize_posted_text(&r.summary, 2000);
    // Stats are recomputed from what is left.
    let mut stats = crate::ai::schemas::ReviewStats::default();
    for c in &r.comments {
        match c.severity.as_str() {
            "error" => stats.errors += 1,
            "warning" => stats.warnings += 1,
            _ => stats.infos += 1,
        }
    }
    r.stats = stats;
    before - r.comments.len()
}

fn validate_inline_comment(
    mut c: InlineComment,
    lines: &HashMap<String, HashSet<u64>>,
) -> Option<InlineComment> {
    let path = c.path.trim().trim_start_matches("b/").to_string();
    if !lines.get(&path).is_some_and(|s| s.contains(&c.line)) {
        return None;
    }
    c.path = path;
    let sev = c.severity.trim().to_lowercase();
    c.severity = if SEVERITIES.contains(&sev.as_str()) {
        sev
    } else {
        "info".to_string()
    };
    let cat = c.category.trim().to_lowercase();
    c.category = if REVIEW_CATEGORIES.contains(&cat.as_str()) {
        cat
    } else {
        "logic".to_string()
    };
    c.body = sanitize_posted_text(&c.body, 1500);
    if c.body.is_empty() {
        return None;
    }
    c.suggestion = c.suggestion.filter(|s| {
        !s.trim().is_empty() && !s.contains("```") && !s.contains("~~~") && s.len() <= 4000
    });
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_and_path_rules() {
        assert!(is_slug("web-ui"));
        assert!(!is_slug("Web UI"));
        assert!(!is_slug(""));
        assert_eq!(sanitize_path("./src/a.rs").as_deref(), Some("src/a.rs"));
        assert!(sanitize_path("../etc/passwd").is_none());
        assert!(sanitize_path("src/`rm -rf`.rs").is_none());
        assert!(sanitize_path("/abs").is_none());
    }

    #[test]
    fn referenced_numbers_reads_hash_refs() {
        let r = referenced_numbers("Fixes #12, see #7 and issue 9");
        assert!(r.contains(&12) && r.contains(&7) && !r.contains(&9));
    }
}
