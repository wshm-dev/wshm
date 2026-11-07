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
use super::{ItemKind, RunContext, StepStatus};

/// The outcome of the last complete review of a PR commit, kept so a run
/// without a review (an approval event, a round cap) still knows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredReview {
    pub head_sha: String,
    pub important: u32,
    pub security_important: bool,
}

fn review_key(n: u64) -> String {
    format!("pipeline_review_state:{n}")
}

/// The stored review of PR `n`, when it is about commit `head`.
pub fn stored_review(ctx: &RunContext, n: u64, head: &str) -> Option<StoredReview> {
    ctx.db
        .get_app_setting(&review_key(n))
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<StoredReview>(&s).ok())
        .filter(|r| r.head_sha == head)
}

fn store_review(ctx: &RunContext, n: u64, r: &StoredReview) {
    match serde_json::to_string(r) {
        Ok(j) => {
            if let Err(e) = ctx.db.set_app_setting(&review_key(n), &j) {
                warn!("Could not store the review state of PR #{n}: {e:#}");
            }
        }
        Err(e) => warn!("Could not encode the review state of PR #{n}: {e:#}"),
    }
}
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
        let security_important = params::bool_or(&def.with, "security_high", true) && {
            let in_run = ctx
                .state()
                .findings
                .iter()
                .any(|f| f.source == "security" && f.severity == Severity::Important);
            let stored = match (ctx.item.number, ctx.review_head().await) {
                (Some(n), Some(h)) => {
                    stored_review(ctx, n, &h).is_some_and(|r| r.security_important)
                }
                _ => false,
            };
            in_run || stored
        };

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
/// approvals on the head commit by humans with write access count (GitHub
/// `author_association` OWNER / MEMBER / COLLABORATOR, Gitea `official`);
/// bots and the author never do. Optionally keeps `label` on the PR while approvals
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

        let head_now = ctx.review_head().await;
        let approved = if !ctx.gh.supports(Capability::PrApprovals) {
            None
        } else {
            match ctx.gh.fetch_pr_approvals(n).await {
                Ok(list) => {
                    let me = ctx.gh.self_username().await;
                    let head = head_now.as_deref();
                    Some(
                        list.iter()
                            .filter(|a| a.write_access && !a.bot)
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

/// `status_check`: a commit status on the PR head the review saw (`context`,
/// default `wshm/review`). Always set (in apply mode, on forges with
/// [`Capability::CommitStatus`], item not flagged), so a required check
/// never stays missing: pending while the review of this commit is not
/// complete (review skipped or failed, round cap, findings not verified),
/// failure while important findings stay open (the verified ones when
/// `verify` ran, else the last stored review of the same commit), pending
/// while required human approvals are missing or could not be counted
/// (`approvals`, default true), success otherwise. Run it with `after`
/// (not `needs`) so it runs whatever the review steps did.
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
        let Some(n) = ctx.item.number else {
            return Ok(StepOutput::skipped("no PR number"));
        };
        let head = ctx.review_head().await;
        let review_posted = {
            let s = ctx.state();
            !s.reviewed
                && s.outcomes
                    .iter()
                    .any(|o| o.uses == "review" && o.status == StepStatus::Done)
        };
        if review_posted {
            super::review_steps::record_posted_review(ctx, n).await;
        }
        let (reviewed, important, security_important, incomplete, approval, outcomes) = {
            let s = ctx.state();
            (
                s.reviewed || s.verified.is_some(),
                s.important_open() as u32,
                s.findings
                    .iter()
                    .any(|f| f.source == "security" && f.severity == Severity::Important),
                s.verify_incomplete.clone(),
                s.approval.clone(),
                s.outcomes.clone(),
            )
        };

        // What the review of this commit says: open important findings, or
        // why it is not complete. A review step of this run that did not
        // finish makes this run's findings partial: the last complete
        // review of the same commit decides, else the status is pending.
        let problem = outcomes
            .iter()
            .find(|o| {
                matches!(o.uses.as_str(), "review" | "verify" | "security")
                    && o.status != StepStatus::Done
            })
            .map(|o| {
                format!(
                    "{} {}: {}",
                    o.id,
                    o.status.as_str(),
                    o.reason.as_deref().unwrap_or("no reason")
                )
            });
        let review: Result<u32, String> = if let Some(why) = incomplete {
            Err(why)
        } else if reviewed && problem.is_none() {
            if let Some(h) = &head {
                store_review(
                    ctx,
                    n,
                    &StoredReview {
                        head_sha: h.clone(),
                        important,
                        security_important,
                    },
                );
            }
            Ok(important)
        } else if let Some(r) = head.as_deref().and_then(|h| stored_review(ctx, n, h)) {
            Ok(r.important)
        } else {
            Err(problem.unwrap_or_else(|| "no review ran on this commit".into()))
        };

        // Approvals: required ones that are missing or could not be
        // counted keep the status pending (never success by default).
        let approval_problem = if !params::bool_or(&def.with, "approvals", true) {
            None
        } else {
            match &approval {
                Some(a) => match a.missing() {
                    Some(0) => None,
                    Some(m) => Some(format!("waiting for {m} more human approval(s)")),
                    None if a.required == 0 => None,
                    None => Some(format!(
                        "approvals could not be counted ({} required)",
                        a.required
                    )),
                },
                None => outcomes.iter().find(|o| o.uses == "approval").map(|o| {
                    format!(
                        "approvals could not be counted (approval step {}: {})",
                        o.status.as_str(),
                        o.reason.as_deref().unwrap_or("no reason")
                    )
                }),
            }
        };

        let (state, description) = match (review, approval_problem) {
            (Err(why), _) => (
                CommitState::Pending,
                format!("review not complete on this commit: {why}"),
            ),
            (Ok(i), _) if i > 0 => (
                CommitState::Failure,
                format!("{i} important finding(s) open"),
            ),
            (Ok(_), Some(p)) => (CommitState::Pending, p),
            (Ok(_), None) => (
                CommitState::Success,
                "no important findings open".to_string(),
            ),
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
        let Some(sha) = head else {
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
        Ok(
            StepOutput::done(format!("{what} (on {})", &sha[..sha.len().min(7)]))
                .with_findings(important),
        )
    }
}
