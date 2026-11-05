//! Step catalog: the [`Step`] trait and the built-in steps.
//!
//! A step kind is registered under a name (`uses = "..."` in a template).
//! The built-in kinds wrap the existing pipelines unchanged; Pro registers
//! extra kinds through [`crate::pro_hooks::set_pipeline_steps`].

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, warn};

use super::template::StepDef;
use super::{ItemKind, RunContext};
use crate::github::sync as gh_sync;

/// What a step reports when it did not fail. A failure is an `Err`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StepOutput {
    /// `Some(reason)` when the step decided not to act (recorded as skipped).
    pub skipped: Option<String>,
    /// Short human summary of what the step did.
    pub summary: Option<String>,
    /// Number of findings (review comments, ...).
    pub findings: u32,
}

impl StepOutput {
    pub fn done(summary: impl Into<String>) -> Self {
        Self {
            summary: Some(summary.into()),
            ..Default::default()
        }
    }

    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            skipped: Some(reason.into()),
            ..Default::default()
        }
    }

    pub fn with_findings(mut self, n: u32) -> Self {
        self.findings = n;
        self
    }
}

/// A pipeline step kind.
#[async_trait]
pub trait Step: Send + Sync {
    /// Name used in templates (`uses`).
    fn kind(&self) -> &'static str;
    /// Item kinds the step can run on. Steps of a template that do not
    /// support the run's item are left out of the run.
    fn items(&self) -> &'static [ItemKind];
    /// Validate the step's `with` parameters. The default accepts none.
    fn validate_params(
        &self,
        params: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        if params.is_empty() {
            Ok(())
        } else {
            Err(format!("step kind '{}' takes no parameters", self.kind()))
        }
    }
    /// Run the step. AI calls made through `AiBackend` while this future
    /// runs are counted by the engine.
    async fn run(&self, ctx: &RunContext, def: &StepDef) -> anyhow::Result<StepOutput>;
}

/// Step kinds by name.
#[derive(Clone, Default)]
pub struct StepRegistry {
    steps: BTreeMap<&'static str, Arc<dyn Step>>,
}

impl StepRegistry {
    /// Built-in steps only.
    pub fn builtin() -> Self {
        let mut r = Self::default();
        r.register(Arc::new(SyncStep));
        r.register(Arc::new(TriageStep));
        r.register(Arc::new(ReviewStep));
        r.register(Arc::new(AnalyzeStep));
        r.register(Arc::new(MemoryStep));
        r
    }

    /// Built-in steps plus the ones registered by Pro.
    pub fn global() -> Self {
        let mut r = Self::builtin();
        for s in crate::pro_hooks::pipeline_steps() {
            r.register(s);
        }
        r
    }

    /// Add (or replace) a step kind.
    pub fn register(&mut self, step: Arc<dyn Step>) {
        self.steps.insert(step.kind(), step);
    }

    pub fn get(&self, kind: &str) -> Option<&Arc<dyn Step>> {
        self.steps.get(kind)
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.steps.keys().copied().collect()
    }
}

const BOTH: &[ItemKind] = &[ItemKind::Issue, ItemKind::Pr];
const ISSUE: &[ItemKind] = &[ItemKind::Issue];
const PR: &[ItemKind] = &[ItemKind::Pr];

/// `sync`: refresh the item cache from the forge (forced list sync, then a
/// direct fetch when the list endpoint lags). Skipped when the forge does
/// not return the item (deleted, transferred, or a PR seen as an issue).
pub struct SyncStep;

#[async_trait]
impl Step for SyncStep {
    fn kind(&self) -> &'static str {
        "sync"
    }

    fn items(&self) -> &'static [ItemKind] {
        BOTH
    }

    async fn run(&self, ctx: &RunContext, _def: &StepDef) -> anyhow::Result<StepOutput> {
        let slug = ctx.config.repo_slug();
        match ctx.item.kind {
            ItemKind::Issue => {
                gh_sync::sync_issues_now(&*ctx.gh, ctx.db.as_ref()).await?;
                let Some(n) = ctx.item.number else {
                    return Ok(StepOutput::done("issues synced"));
                };
                if matches!(ctx.db.get_issue(n), Ok(None)) {
                    match ctx.gh.fetch_issue(n).await {
                        Ok(Some(issue)) => {
                            if let Err(e) = ctx.db.upsert_issue(&issue) {
                                warn!("Failed to cache issue #{n} after direct fetch: {e}");
                            }
                        }
                        Ok(None) => {
                            info!(
                                "[{slug}] issue #{n} not returned by GitHub (deleted, transferred, or a PR) — skipping"
                            );
                            return Ok(StepOutput::skipped(
                                "issue not returned by the forge (deleted, transferred, or a PR)",
                            ));
                        }
                        Err(e) => {
                            warn!("Direct fetch of issue #{n} failed: {e:#}");
                            return Ok(StepOutput::done(format!(
                                "issues synced; direct fetch failed: {e}"
                            )));
                        }
                    }
                }
                Ok(StepOutput::done("issues synced"))
            }
            ItemKind::Pr => {
                gh_sync::sync_pulls_now(&*ctx.gh, ctx.db.as_ref()).await?;
                let Some(n) = ctx.item.number else {
                    return Ok(StepOutput::done("pull requests synced"));
                };
                if matches!(ctx.db.get_pull(n), Ok(None)) {
                    match ctx.gh.fetch_pull(n).await {
                        Ok(Some(pr)) => {
                            if let Err(e) = ctx.db.upsert_pull(&pr) {
                                warn!("Failed to cache PR #{n} after direct fetch: {e}");
                            }
                        }
                        Ok(None) => {
                            info!(
                                "[{slug}] PR #{n} not returned by GitHub (deleted or transferred) — skipping"
                            );
                            return Ok(StepOutput::skipped(
                                "PR not returned by the forge (deleted or transferred)",
                            ));
                        }
                        Err(e) => {
                            warn!("Direct fetch of PR #{n} failed: {e:#}");
                            return Ok(StepOutput::done(format!(
                                "pull requests synced; direct fetch failed: {e}"
                            )));
                        }
                    }
                }
                Ok(StepOutput::done("pull requests synced"))
            }
        }
    }
}

/// `triage`: the issue triage pipeline (classification, labels, comment,
/// and the auto-fix of `[triage] auto_fix`). Posts only in apply mode.
pub struct TriageStep;

#[async_trait]
impl Step for TriageStep {
    fn kind(&self) -> &'static str {
        "triage"
    }

    fn items(&self) -> &'static [ItemKind] {
        ISSUE
    }

    async fn run(&self, ctx: &RunContext, _def: &StepDef) -> anyhow::Result<StepOutput> {
        let args = crate::cli::TriageArgs {
            issue: ctx.item.number,
            apply: ctx.apply,
            retriage: false,
        };
        crate::pipelines::triage::run(
            &ctx.config,
            ctx.db.as_ref(),
            &*ctx.gh,
            &args,
            crate::pipelines::triage::OutputFormat::Text,
            None,
        )
        .await?;
        let summary = ctx
            .item
            .number
            .and_then(|n| ctx.db.get_triage_result(n).ok().flatten())
            .map(|t| format!("{} (confidence {:.2})", t.category, t.confidence))
            .unwrap_or_else(|| "no triage result".into());
        Ok(StepOutput::done(summary))
    }
}

/// `review`: the inline code review (Pro hook). Computed and stored always;
/// posted only when apply mode and `review_post_comments` are both on.
pub struct ReviewStep;

#[async_trait]
impl Step for ReviewStep {
    fn kind(&self) -> &'static str {
        "review"
    }

    fn items(&self) -> &'static [ItemKind] {
        PR
    }

    async fn run(&self, ctx: &RunContext, _def: &StepDef) -> anyhow::Result<StepOutput> {
        let Some(n) = ctx.item.number else {
            return Ok(StepOutput::skipped("no PR number"));
        };
        let before = ctx
            .db
            .get_pr_review(n)
            .ok()
            .flatten()
            .map(|r| r.reviewed_at);
        let should_post = ctx.apply && ctx.features.review_post_comments;
        let ran =
            crate::pro_hooks::run_review(&ctx.config, ctx.db.as_ref(), &*ctx.gh, n, should_post)
                .await
                .inspect_err(|e| warn!("Review failed for PR #{n}: {e:#}"))?;
        if !ran {
            return Ok(StepOutput::skipped("inline review requires wshm Pro"));
        }
        match ctx.db.get_pr_review(n).ok().flatten() {
            Some(r) if Some(&r.reviewed_at) != before.as_ref() => {
                let count = r.result.comments.len() as u32;
                let posted = if r.posted_to_github {
                    "posted"
                } else {
                    "not posted"
                };
                Ok(StepOutput::done(format!("{count} comment(s), {posted}")).with_findings(count))
            }
            Some(r) => Ok(StepOutput::done("unchanged since the last review")
                .with_findings(r.result.comments.len() as u32)),
            None => Ok(StepOutput::done("no review stored")),
        }
    }
}

/// `analyze`: the PR analysis (type, risk, labels, comment). Diagram jobs
/// it returns run in the background, outside the run.
pub struct AnalyzeStep;

#[async_trait]
impl Step for AnalyzeStep {
    fn kind(&self) -> &'static str {
        "analyze"
    }

    fn items(&self) -> &'static [ItemKind] {
        PR
    }

    async fn run(&self, ctx: &RunContext, _def: &StepDef) -> anyhow::Result<StepOutput> {
        use crate::pipelines::pr_analysis;
        let args = crate::cli::PrArgs {
            pr: ctx.item.number,
            apply: ctx.apply,
        };
        let jobs = pr_analysis::run_with(
            &ctx.config,
            ctx.db.as_ref(),
            &*ctx.gh,
            &args,
            false,
            None,
            pr_analysis::RunOptions {
                preview: false,
                diagrams: pr_analysis::DiagramMode::Deferred,
            },
        )
        .await?;
        let n_jobs = jobs.len();
        for job in jobs {
            pr_analysis::spawn_diagram_job(
                Arc::clone(&ctx.config),
                Arc::clone(&ctx.db),
                Arc::clone(&ctx.gh),
                job,
            );
        }
        let mut summary = ctx
            .item
            .number
            .and_then(|n| ctx.db.get_pr_analysis(n).ok().flatten())
            .map(|a| format!("{}, risk {}", a.pr_type, a.risk_level))
            .unwrap_or_else(|| "no analysis result".into());
        if n_jobs > 0 {
            summary.push_str("; diagram queued");
        }
        Ok(StepOutput::done(summary))
    }
}

/// `memory`: store the triage / analysis result in ICM
/// (`[daemon] icm_enabled`).
pub struct MemoryStep;

#[async_trait]
impl Step for MemoryStep {
    fn kind(&self) -> &'static str {
        "memory"
    }

    fn items(&self) -> &'static [ItemKind] {
        BOTH
    }

    async fn run(&self, ctx: &RunContext, _def: &StepDef) -> anyhow::Result<StepOutput> {
        if !ctx.config.daemon.icm_enabled {
            return Ok(StepOutput::skipped("[daemon] icm_enabled is off"));
        }
        let Some(n) = ctx.item.number else {
            return Ok(StepOutput::skipped("no item number"));
        };
        match ctx.item.kind {
            ItemKind::Issue => match ctx.db.get_triage_result(n) {
                Ok(Some(t)) => {
                    crate::daemon::memory::store_triage(
                        &ctx.config,
                        n,
                        &t.category,
                        t.confidence,
                        t.summary.as_deref().unwrap_or(""),
                    )
                    .await;
                    Ok(StepOutput::done("triage stored in ICM"))
                }
                _ => Ok(StepOutput::skipped("no triage result to store")),
            },
            ItemKind::Pr => match ctx.db.get_pr_analysis(n) {
                Ok(Some(a)) => {
                    crate::daemon::memory::store_pr_analysis(
                        &ctx.config,
                        n,
                        &a.pr_type,
                        &a.risk_level,
                        &a.summary,
                    )
                    .await;
                    Ok(StepOutput::done("analysis stored in ICM"))
                }
                _ => Ok(StepOutput::skipped("no analysis to store")),
            },
        }
    }
}
