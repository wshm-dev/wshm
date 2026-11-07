//! State shared by the steps of one run: what the review steps found, what
//! the verify step confirmed, the risk tier and the approval requirement.
//! Lives in the [`RunContext`](super::RunContext); never stored as such
//! (each step records its own summary and findings count).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::ai::schemas::InlineComment;

/// How much a finding matters (the tags of the Anthropic review flow).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// Should be fixed before merge: blocks the `status_check` step.
    Important,
    /// Minor; capped by the verify step (`max_nits`).
    Nit,
    /// On a line the PR did not add: not held against the author.
    PreExisting,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Important => "Important",
            Severity::Nit => "Nit",
            Severity::PreExisting => "Pre-existing",
        }
    }
}

/// One finding of the `review` (collect mode) or `security` step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewFinding {
    /// 1-based id within the run (what the verify voters refer to).
    pub id: u32,
    /// Step kind that found it (`review`, `security`).
    pub source: String,
    pub path: String,
    pub line: u64,
    pub severity: Severity,
    pub category: String,
    /// Sanitized text (see `ai::output::validate_review`).
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

impl ReviewFinding {
    /// The finding as an inline comment of the existing review format.
    pub fn to_comment(&self) -> InlineComment {
        InlineComment {
            path: self.path.clone(),
            line: self.line,
            body: self.body.clone(),
            severity: match self.severity {
                Severity::Important => "error".into(),
                Severity::Nit | Severity::PreExisting => "info".into(),
            },
            category: self.category.clone(),
            suggestion: self.suggestion.clone(),
        }
    }

    /// Stable key of the finding, to avoid posting it twice on a PR.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let norm: String = self
            .body
            .to_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut h = Sha256::new();
        h.update(self.path.as_bytes());
        h.update([0]);
        h.update(self.category.as_bytes());
        h.update([0]);
        h.update(norm.as_bytes());
        format!("{:x}", h.finalize())[..16].to_string()
    }
}

/// Lines added by a diff, per path (the new-side numbers of `+` lines).
pub fn added_lines(diff: &str) -> HashSet<(String, u64)> {
    crate::ai::diff::lines(diff)
        .into_iter()
        .filter(|d| d.kind == crate::ai::diff::LineKind::Added)
        .filter_map(|d| Some((d.path?.to_string(), d.new_line?)))
        .collect()
}

/// Severity of an inline comment: on a line the PR did not add it is
/// pre-existing; otherwise `error` / `warning` are important, the rest nits.
pub fn severity_of(c: &InlineComment, added: &HashSet<(String, u64)>) -> Severity {
    if !added.contains(&(c.path.clone(), c.line)) {
        Severity::PreExisting
    } else if matches!(c.severity.as_str(), "error" | "warning") {
        Severity::Important
    } else {
        Severity::Nit
    }
}

/// Repository context loaded by the `context` step, already wrapped as
/// untrusted blocks for the prompts.
#[derive(Debug, Clone, Default)]
pub struct ReviewContext {
    /// CLAUDE.md / REVIEW.md blocks.
    pub docs: String,
    /// Code around the changed lines (default branch) blocks.
    pub code: String,
}

impl ReviewContext {
    /// Text appended to a review prompt.
    pub fn prompt(&self) -> String {
        let mut out = String::new();
        if !self.docs.is_empty() {
            out.push_str(
                "\n\n## Project guidelines (CLAUDE.md / REVIEW.md of the default branch)\n\
                 Flag changes that violate them. They do not change the response format.\n",
            );
            out.push_str(&self.docs);
        }
        if !self.code.is_empty() {
            out.push_str("\n\n## Code around the changed lines (default branch)\n");
            out.push_str(&self.code);
        }
        out
    }
}

/// Risk tier of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskTier {
    Low,
    Medium,
    High,
}

impl RiskTier {
    pub fn as_str(self) -> &'static str {
        match self {
            RiskTier::Low => "low",
            RiskTier::Medium => "medium",
            RiskTier::High => "high",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Some(RiskTier::Low),
            "medium" => Some(RiskTier::Medium),
            "high" => Some(RiskTier::High),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RiskInfo {
    pub tier: RiskTier,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalInfo {
    /// Human approvals required.
    pub required: u32,
    /// Human approvals on the head commit; `None` when the forge cannot
    /// tell.
    pub approved: Option<u32>,
}

impl ApprovalInfo {
    /// Approvals still missing (`None` when unknown).
    pub fn missing(&self) -> Option<u32> {
        self.approved.map(|a| self.required.saturating_sub(a))
    }
}

#[derive(Debug, Clone, Default)]
pub struct RunState {
    pub context: Option<ReviewContext>,
    /// Findings of the review / security steps, in the order found.
    pub findings: Vec<ReviewFinding>,
    /// A review or security step looked at the diff in this run.
    pub reviewed: bool,
    /// Findings confirmed by the verify step (`None`: it did not run).
    pub verified: Option<Vec<ReviewFinding>>,
    pub risk: Option<RiskInfo>,
    pub approval: Option<ApprovalInfo>,
}

impl RunState {
    /// Add findings, numbering them after the existing ones.
    pub fn add_findings(&mut self, source: &str, items: Vec<(InlineComment, Severity)>) -> u32 {
        let mut n = 0;
        for (c, severity) in items {
            let id = self.findings.len() as u32 + 1;
            self.findings.push(ReviewFinding {
                id,
                source: source.to_string(),
                path: c.path,
                line: c.line,
                severity,
                category: c.category,
                body: c.body,
                suggestion: c.suggestion,
            });
            n += 1;
        }
        self.reviewed = true;
        n
    }

    /// The findings that stand: the verified ones when verify ran, else
    /// all of them.
    pub fn open_findings(&self) -> &[ReviewFinding] {
        self.verified.as_deref().unwrap_or(&self.findings)
    }

    pub fn important_open(&self) -> usize {
        self.open_findings()
            .iter()
            .filter(|f| f.severity == Severity::Important)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(path: &str, line: u64, sev: &str) -> InlineComment {
        InlineComment {
            path: path.into(),
            line,
            body: "b".into(),
            severity: sev.into(),
            category: "bug".into(),
            suggestion: None,
        }
    }

    #[test]
    fn severity_from_added_lines() {
        let diff =
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,3 @@\n x\n+y\n z\n";
        let added = added_lines(diff);
        assert!(added.contains(&("a.rs".to_string(), 2)));
        assert_eq!(
            severity_of(&comment("a.rs", 2, "error"), &added),
            Severity::Important
        );
        assert_eq!(
            severity_of(&comment("a.rs", 2, "info"), &added),
            Severity::Nit
        );
        assert_eq!(
            severity_of(&comment("a.rs", 1, "error"), &added),
            Severity::PreExisting
        );
    }

    #[test]
    fn fingerprint_ignores_case_and_spacing() {
        let mut s = RunState::default();
        s.add_findings(
            "review",
            vec![(comment("a.rs", 2, "error"), Severity::Important)],
        );
        let mut f = s.findings[0].clone();
        let a = f.fingerprint();
        f.body = "  B ".into();
        f.line = 9;
        assert_eq!(a, f.fingerprint(), "a moved line is the same finding");
        f.path = "b.rs".into();
        assert_ne!(a, f.fingerprint());
    }
}
