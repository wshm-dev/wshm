//! Merge-gate steps: `risk` (classify the change by paths and size),
//! `approval` (human approvals required per risk tier) and `status_check`
//! (a commit status that blocks the merge while important findings stay
//! open). None of them calls the AI.

use async_trait::async_trait;
use tracing::warn;

use super::params::{self, Param};
use super::state::{ApprovalInfo, RiskInfo, RiskTier, Severity};
use super::steps::{Step, StepOutput};
use super::template::StepDef;
use super::{ItemKind, RunContext};
use crate::git_provider::{Capability, CommitState, CommitStatus};

const PR: &[ItemKind] = &[ItemKind::Pr];

/// Paths that make a change high risk by default.
pub const DEFAULT_HIGH_PATHS: &[&str] = &[
    ".github/workflows/**",
    ".gitlab-ci.yml",
    "**/Dockerfile",
    "**/migrations/**",
    "**/auth/**",
    "**/security/**",
    "**/*.pem",
];
/// Paths that make a change low risk by default (when all paths match).
pub const DEFAULT_LOW_PATHS: &[&str] = &["**/*.md", "docs/**", "**/*.txt", "**/*.rst", "LICENSE*"];
/// Paths that always need a human approval by default.
pub const DEFAULT_ALWAYS_HUMAN: &[&str] = &[".github/workflows/**", "**/CODEOWNERS"];

fn list(paths: &[String], max: usize) -> String {
    let mut s = paths
        .iter()
        .take(max)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > max {
        s.push_str(&format!(" (+{})", paths.len() - max));
    }
    s
}

const RISK_PARAMS: &[(&str, Param)] = &[
    ("high_paths", Param::Globs),
    ("low_paths", Param::Globs),
    ("large_lines", Param::Int(1, 1_000_000)),
    ("security_high", Param::Bool),
];

/// `risk`: low when every changed path is a low-risk path (docs), high
/// when a path is sensitive, the change is large, or an important security
/// finding was made in this run; medium otherwise. The tier is what
/// `when.risk` and the `approval` step read.
pub struct RiskStep;

#[async_trait]
impl Step for RiskStep {
    fn kind(&self) -> &'static str {
        "risk"
    }

    fn items(&self) -> &'static [ItemKind] {
        PR
    }

    fn mutating(&self) -> bool {
        false
    }

    fn validate_params(
        &self,
        p: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        params::check(self.kind(), p, RISK_PARAMS)
    }

    async fn run(&self, ctx: &RunContext, def: &StepDef) -> anyhow::Result<StepOutput> {
        let diff = ctx.pr_diff().await.map_err(anyhow::Error::msg)?;
        let paths = ctx.changed_paths().await.map_err(anyhow::Error::msg)?;
        let lines = crate::pipelines::review::compute_diff_size(&diff).total_lines();
        let high = params::list_or(&def.with, "high_paths", DEFAULT_HIGH_PATHS);
        let low = params::list_or(&def.with, "low_paths", DEFAULT_LOW_PATHS);
        let large = params::u64_or(&def.with, "large_lines", 1000) as usize;
        let security_important = params::bool_or(&def.with, "security_high", true)
            && ctx
                .state()
                .findings
                .iter()
                .any(|f| f.source == "security" && f.severity == Severity::Important);

        let sensitive: Vec<String> = paths
            .iter()
            .filter(|p| params::matches_any(&high, p))
            .cloned()
            .collect();
        let (tier, reason) = if !sensitive.is_empty() {
            (
                RiskTier::High,
                format!("sensitive path(s): {}", list(&sensitive, 3)),
            )
        } else if lines > large {
            (
                RiskTier::High,
                format!("{lines} changed lines (more than {large})"),
            )
        } else if security_important {
            (RiskTier::High, "important security finding".to_string())
        } else if !paths.is_empty() && paths.iter().all(|p| params::matches_any(&low, p)) {
            (RiskTier::Low, "only low-risk paths (docs)".to_string())
        } else {
            (
                RiskTier::Medium,
                format!("{} file(s), {lines} changed lines", paths.len()),
            )
        };
        ctx.state().risk = Some(RiskInfo {
            tier,
            reasons: vec![reason.clone()],
        });
        Ok(StepOutput::done(format!("{}: {reason}", tier.as_str())))
    }
}

const APPROVAL_PARAMS: &[(&str, Param)] = &[
    ("low", Param::Int(0, 10)),
    ("medium", Param::Int(0, 10)),
    ("high", Param::Int(0, 10)),
    ("always_human", Param::Globs),
    ("trusted_authors", Param::List),
    ("label", Param::Str(50)),
];

/// `approval`: human approvals required for the PR, by risk tier (default
/// 0 / 1 / 2), at least one when a changed path is in `always_human` or
/// the author is not in `trusted_authors` (when that list is set). Only
/// human approvals on the head commit count: bots, wshm's own account and
/// the author do not. Optionally keeps `label` on the PR while approvals
/// are missing (apply mode).
pub struct ApprovalStep;

#[async_trait]
impl Step for ApprovalStep {
    fn kind(&self) -> &'static str {
        "approval"
    }

    fn items(&self) -> &'static [ItemKind] {
        PR
    }

    fn validate_params(
        &self,
        p: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        params::check(self.kind(), p, APPROVAL_PARAMS)
    }

    async fn run(&self, ctx: &RunContext, def: &StepDef) -> anyhow::Result<StepOutput> {
        let Some(n) = ctx.item.number else {
            return Ok(StepOutput::skipped("no PR number"));
        };
        let tier = ctx.state().risk.as_ref().map(|r| r.tier).or_else(|| {
            ctx.db
                .get_pr_analysis(n)
                .ok()
                .flatten()
                .and_then(|a| RiskTier::parse(&a.risk_level))
        });
        let Some(tier) = tier else {
            return Ok(StepOutput::skipped(
                "no risk tier: run a risk (or analyze) step first",
            ));
        };
        let w = &def.with;
        let mut required = match tier {
            RiskTier::Low => params::u64_or(w, "low", 0),
            RiskTier::Medium => params::u64_or(w, "medium", 1),
            RiskTier::High => params::u64_or(w, "high", 2),
        } as u32;
        let mut why = vec![format!("risk {}", tier.as_str())];

        let always = params::list_or(w, "always_human", DEFAULT_ALWAYS_HUMAN);
        let paths = ctx.changed_paths().await.unwrap_or_default();
        let human_paths: Vec<String> = paths
            .iter()
            .filter(|p| params::matches_any(&always, p))
            .cloned()
            .collect();
        if !human_paths.is_empty() && required == 0 {
            required = 1;
            why.push(format!("always needs a human: {}", list(&human_paths, 2)));
        }
        let info = ctx.item_info().await.unwrap_or_default();
        let author = info.author.clone().unwrap_or_default();
        let trusted = params::list_or(w, "trusted_authors", &[]);
        if !trusted.is_empty()
            && !trusted.iter().any(|t| t.eq_ignore_ascii_case(&author))
            && required == 0
        {
            required = 1;
            why.push(format!("author '{author}' not in trusted_authors"));
        }

        let approved = if !ctx.gh.supports(Capability::PrApprovals) {
            None
        } else {
            match ctx.gh.fetch_pr_approvals(n).await {
                Ok(list) => {
                    let me = ctx.gh.self_username().await;
                    let head = info.head_sha.as_deref();
                    Some(
                        list.iter()
                            .filter(|a| !a.bot)
                            .filter(|a| !a.login.eq_ignore_ascii_case(&author))
                            .filter(|a| {
                                me.as_deref()
                                    .is_none_or(|m| !a.login.eq_ignore_ascii_case(m))
                            })
                            .filter(|a| match (head, a.commit_sha.as_deref()) {
                                (Some(h), Some(c)) => h == c,
                                _ => true,
                            })
                            .count() as u32,
                    )
                }
                Err(e) => {
                    warn!("Could not list the approvals of PR #{n}: {e:#}");
                    None
                }
            }
        };
        let a = ApprovalInfo { required, approved };
        ctx.state().approval = Some(a.clone());

        let mut summary = format!("{required} human approval(s) required ({})", why.join("; "));
        match approved {
            Some(k) => summary.push_str(&format!(", {k} on the head commit")),
            None => summary.push_str(&format!(
                ", approvals not counted on {}",
                ctx.gh.provider_name()
            )),
        }

        let label = params::str_or(w, "label", "").trim().to_string();
        if !label.is_empty() {
            if let Some(missing) = a.missing() {
                summary.push_str(
                    &self
                        .sync_label(ctx, n, &label, missing > 0, &info.labels)
                        .await,
                );
            }
        }
        Ok(StepOutput::done(summary))
    }
}

impl ApprovalStep {
    /// Add or remove `label` (apply mode, item not flagged).
    async fn sync_label(
        &self,
        ctx: &RunContext,
        n: u64,
        label: &str,
        want: bool,
        labels: &[String],
    ) -> String {
        let has = labels.iter().any(|l| l.eq_ignore_ascii_case(label));
        if want == has {
            return String::new();
        }
        if !ctx.apply {
            return format!("; dry-run: label '{label}' not changed");
        }
        if ctx.mutations_blocked() {
            return "; flagged as possible prompt injection: label not changed".into();
        }
        let res = if want {
            ctx.gh.label_pr(n, &[label.to_string()]).await
        } else {
            ctx.gh.remove_pr_label(n, label).await
        };
        match res {
            Ok(()) if want => format!("; label '{label}' added"),
            Ok(()) => format!("; label '{label}' removed"),
            Err(e) => {
                warn!("Could not update label '{label}' on PR #{n}: {e:#}");
                format!("; label update failed: {e}")
            }
        }
    }
}

const STATUS_PARAMS: &[(&str, Param)] = &[("context", Param::Str(100)), ("approvals", Param::Bool)];

/// `status_check`: a commit status on the PR head (`context`, default
/// `wshm/review`): failure while important findings of the run stay open
/// (the verified ones when `verify` ran), pending while human approvals
/// are missing (`approvals`, default true), success otherwise. Branch
/// protection can require it. Only on forges with
/// [`Capability::CommitStatus`], in apply mode.
pub struct StatusCheckStep;

#[async_trait]
impl Step for StatusCheckStep {
    fn kind(&self) -> &'static str {
        "status_check"
    }

    fn items(&self) -> &'static [ItemKind] {
        PR
    }

    fn validate_params(
        &self,
        p: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        params::check(self.kind(), p, STATUS_PARAMS)
    }

    async fn run(&self, ctx: &RunContext, def: &StepDef) -> anyhow::Result<StepOutput> {
        if !ctx.gh.supports(Capability::CommitStatus) {
            return Ok(StepOutput::skipped(format!(
                "{} does not support commit statuses",
                ctx.gh.provider_name()
            )));
        }
        let (reviewed, important, approval) = {
            let s = ctx.state();
            (
                s.reviewed || s.verified.is_some(),
                s.important_open() as u32,
                s.approval.clone(),
            )
        };
        if !reviewed {
            return Ok(StepOutput::skipped("no review in this run"));
        }
        let missing = if params::bool_or(&def.with, "approvals", true) {
            approval.and_then(|a| a.missing()).unwrap_or(0)
        } else {
            0
        };
        let (state, description) = if important > 0 {
            (
                CommitState::Failure,
                format!("{important} important finding(s) open"),
            )
        } else if missing > 0 {
            (
                CommitState::Pending,
                format!("waiting for {missing} more human approval(s)"),
            )
        } else {
            (
                CommitState::Success,
                "no important findings open".to_string(),
            )
        };
        let context = params::str_or(&def.with, "context", "wshm/review").to_string();
        let what = format!("{context} = {}: {description}", state.as_str());
        if !ctx.apply {
            return Ok(StepOutput::skipped(format!("dry-run: would set {what}")));
        }
        if ctx.mutations_blocked() {
            return Ok(StepOutput::skipped(
                "flagged as possible prompt injection: status not set",
            ));
        }
        let Some(sha) = ctx.item_info().await.and_then(|i| i.head_sha) else {
            return Ok(StepOutput::skipped("PR head commit unknown"));
        };
        ctx.gh
            .set_commit_status(
                &sha,
                &CommitStatus {
                    state,
                    context,
                    description,
                },
            )
            .await?;
        Ok(StepOutput::done(what).with_findings(important))
    }
}
