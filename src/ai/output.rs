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
// `[text](target)` and `![alt](target)`, also split across lines.
static RE_MD_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)!?\[([^\]]*)\]\s*\(([^)]*)\)").unwrap());
// Reference definitions, also inside quotes and list items.
static RE_MD_REFDEF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*(?:(?:>|[-*+]|\d+[.)])[ \t]*)*\[[^\]\n]+\]:[^\n]*$").unwrap()
});
static RE_MD_REFLINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!?\[([^\]\n]+)\]\[[^\]\n]*\]").unwrap());
static RE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:https?|ftp|file|ssh|git|ws|wss)\s*:\s*//\S+|(?:^|[\s(<\x22'])//[a-z0-9-]+(?:\.[a-z0-9-]+)+\S*|\bwww\.[a-z0-9-]+\.\S+",
    )
    .unwrap()
});
static RE_SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:javascript|vbscript|data)\s*:").unwrap());
static RE_MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^A-Za-z0-9_])@([A-Za-z0-9][A-Za-z0-9_/.-]*)").unwrap());
// `@` written as an entity: `&#64;`, `&#x40;`, `&commat;`.
static RE_AT_ENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)&(?:#0*64|#x0*40|commat);?").unwrap());

/// Obvious secret shapes, redacted from posted AI text (defence in depth:
/// the model should never see secrets, but if it echoes one it is not
/// published).
static RE_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?(?:-----END [A-Z ]*PRIVATE KEY-----|$)",
        r"|\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}",
        r"|\bgithub_pat_[A-Za-z0-9_]{20,}",
        r"|\bglpat-[A-Za-z0-9_-]{20,}",
        r"|\bsk-ant-[A-Za-z0-9_-]{20,}",
        r"|\bsk-(?:proj-)?[A-Za-z0-9_-]{32,}",
        r"|\bxox[abposr]-[A-Za-z0-9-]{10,}",
        r"|\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        r"|\bAIza[0-9A-Za-z_-]{35}\b",
        r"|\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
        r"|(?i:\b(?:api[_-]?key|secret|token|password|passwd)\b\s*[:=]\s*['\x22]?)[A-Za-z0-9_\-/+=.]{16,}",
    ))
    .unwrap()
});

/// Redact secret-looking strings.
pub fn redact_secrets(s: &str) -> String {
    RE_SECRET.replace_all(s, "[secret removed]").into_owned()
}

/// Neutralise `@user` / `@org/team` mentions (a zero-width space after the
/// `@` keeps the text readable but does not notify anyone). Entity-encoded
/// `@` is decoded first so it cannot slip through.
pub fn neutralize_mentions(s: &str) -> String {
    let s = RE_AT_ENTITY.replace_all(s, "@");
    RE_MENTION.replace_all(&s, "$1@\u{200B}$2").into_owned()
}

/// Escape HTML in prose: `<` becomes `&lt;`, so no tag can render and the
/// forge shows it as text (nothing is deleted; `>` stays, it is harmless
/// alone and starts Markdown quotes).
fn escape_html(s: &str) -> String {
    s.replace('<', "&lt;")
}

fn sanitize_prose(p: &str) -> String {
    let mut out = RE_HTML_COMMENT.replace_all(p, "").into_owned();
    out = RE_MD_LINK.replace_all(&out, "$1").into_owned();
    out = RE_MD_REFDEF.replace_all(&out, "").into_owned();
    out = RE_MD_REFLINK.replace_all(&out, "$1").into_owned();
    out = RE_URL
        .replace_all(&out, |c: &regex::Captures| {
            // Keep the separator a protocol-relative match started with.
            let m = &c[0];
            let lead: String = m
                .chars()
                .take_while(|ch| !ch.is_alphanumeric() && *ch != '/')
                .collect();
            format!("{lead}[link removed]")
        })
        .into_owned();
    out = RE_SCHEME.replace_all(&out, "").into_owned();
    out = neutralize_mentions(&out);
    escape_html(&out)
}

/// Make AI-written text safe to post on the forge. Prose loses HTML
/// comments, links (Markdown, split, reference-style, bare, protocol-relative,
/// dangerous schemes) and mentions; remaining HTML is escaped, not deleted.
/// Code spans and fenced blocks are kept verbatim (`Vec<String>`, `a<b`):
/// the forge renders them as text, links and mentions do not fire there.
/// Invisible characters and secret-looking strings are removed everywhere.
/// The result is capped at `max_chars` characters.
pub fn sanitize_posted_text(s: &str, max_chars: usize) -> String {
    let s = crate::ai::untrusted::strip_invisible_text(s);
    let s = redact_secrets(&s);
    let out = crate::ai::markdown::map_prose(&s, sanitize_prose);
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

/// Labels listed in the default triage prompt: the allowlist when no
/// `[[labels]]` are configured.
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

/// Whether `label` looks like a CI, merge or deploy trigger (bots act on
/// these: `ok-to-test`, `automerge`, `lgtm`, `run-ci`, ...). The model may
/// never apply one to a PR, and only an explicitly configured one to an
/// issue.
pub fn is_trigger_label(label: &str) -> bool {
    let l = label.trim().to_lowercase().replace([' ', '_'], "-");
    const EXACT: &[&str] = &[
        "ok-to-test",
        "safe-to-test",
        "automerge",
        "auto-merge",
        "autosquash",
        "auto-squash",
        "lgtm",
        "approved",
        "approve",
        "merge",
        "ready-to-merge",
        "merge-when-ready",
        "mergeable",
        "ship-it",
        "shipit",
        "run-ci",
        "ci",
        "skip-ci",
        "ci-skip",
        "deploy",
        "release",
        "publish",
        "trusted",
        "safe",
        "verified",
    ];
    EXACT.contains(&l.as_str())
        || l.ends_with("-to-test")
        || l.ends_with("-to-merge")
        || l.starts_with("run-")
        || l.starts_with("ci:")
        || l.starts_with("ci/")
        || l.starts_with("deploy")
        || l.starts_with("merge")
        || l.starts_with("automerge")
        || l.starts_with("auto-merge")
        || l.starts_with("kokoro:")
        || l.starts_with("approved")
}

/// Where a label would be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelTarget {
    Issue,
    PullRequest,
}

/// Which labels the model may apply.
#[derive(Debug, Clone)]
pub struct LabelPolicy {
    /// Lowercased allowed names.
    allowed: HashSet<String>,
    /// The allowlist comes from `[[labels]]` (explicit).
    configured: bool,
    /// Lowercased names the model may never apply (approval label, markers).
    forbidden: HashSet<String>,
    target: LabelTarget,
}

impl Default for LabelPolicy {
    fn default() -> Self {
        Self::new(std::iter::empty(), std::iter::empty(), LabelTarget::Issue)
    }
}

impl LabelPolicy {
    /// `configured`: the `[[labels]]` names (exclusive when non-empty); else
    /// [`STANDARD_LABELS`]. Labels merely present in the repository are not
    /// allowed: a CI or merge trigger could be among them.
    pub fn new<'a>(
        configured: impl IntoIterator<Item = &'a str>,
        forbidden: impl IntoIterator<Item = &'a str>,
        target: LabelTarget,
    ) -> Self {
        let configured: HashSet<String> = configured.into_iter().map(str::to_lowercase).collect();
        let is_configured = !configured.is_empty();
        let allowed = if is_configured {
            configured
        } else {
            STANDARD_LABELS.iter().map(|s| s.to_string()).collect()
        };
        Self {
            allowed,
            configured: is_configured,
            forbidden: forbidden
                .into_iter()
                .map(str::to_lowercase)
                .filter(|s| !s.is_empty())
                .collect(),
            target,
        }
    }

    pub fn allows(&self, label: &str) -> bool {
        let l = label.trim().to_lowercase();
        if l.is_empty()
            || self.forbidden.contains(&l)
            // Domain labels are applied by wshm only for validated domains;
            // `wshm:` labels are control markers set by maintainers.
            || l.starts_with("domain:")
            || l.starts_with("wshm:")
            || !self.allowed.contains(&l)
        {
            return false;
        }
        // CI/merge triggers: never on a PR; on an issue only when the
        // maintainers listed it explicitly.
        !is_trigger_label(&l) || (self.target == LabelTarget::Issue && self.configured)
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
    let mut out: HashMap<String, HashSet<u64>> = HashMap::new();
    for d in crate::ai::diff::lines(diff) {
        if let (Some(p), Some(n)) = (d.path, d.new_line) {
            out.entry(p.to_string()).or_default().insert(n);
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
