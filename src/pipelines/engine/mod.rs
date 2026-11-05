//! Configurable pipeline engine (#234).
//!
//! The processing of an issue or PR event is a *run* of a *template*: an
//! ordered list of steps ([`template`]), each gated by conditions
//! ([`condition`]) and implemented by a step kind ([`steps`]). Routes pick
//! the template; the built-in `standard` template reproduces the behaviour
//! wshm had before the engine and is the default.
//!
//! Every run and each of its steps is recorded in the DB
//! (`pipeline_runs` / `pipeline_step_runs`, see [`crate::db::runs`]):
//! status done / skipped (with the reason) / failed, timings, AI calls,
//! a short summary and the number of findings.
//!
//! Steps run sequentially in template order. `needs` already expresses the
//! dependency graph, so a later version can run independent branches in
//! parallel without changing the template format.

pub mod condition;
pub mod steps;
pub mod template;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::config::{Config, RepoFeatures};
use crate::db::backend::DatabaseBackend;
use crate::db::runs::{NewPipelineRun, PipelineRunFinish, PipelineStepRunRow, PlannedStep};
use crate::git_provider::Forge as GhClient;

use self::steps::{StepOutput, StepRegistry};
use self::template::{OnFailure, PipelineTemplate, StepDef};

/// Max length of the stored summary / skip reason / error of a step.
const MAX_TEXT: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Issue,
    Pr,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Issue => "issue",
            ItemKind::Pr => "pr",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "issue" => Some(ItemKind::Issue),
            "pr" => Some(ItemKind::Pr),
            _ => None,
        }
    }
}

/// The issue or PR a run is about. `number` is `None` only for an event
/// whose payload carried no number (then nothing is recorded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemRef {
    pub kind: ItemKind,
    pub number: Option<u64>,
}

impl std::fmt::Display for ItemRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.kind {
            ItemKind::Issue => "issue",
            ItemKind::Pr => "PR",
        };
        match self.number {
            Some(n) => write!(f, "{what} #{n}"),
            None => write!(f, "{what}"),
        }
    }
}

/// Item fields the conditions look at.
#[derive(Debug, Clone, Default)]
pub struct ItemInfo {
    pub labels: Vec<String>,
    pub author: Option<String>,
    pub head_sha: Option<String>,
}

/// Everything a step can use. Built once per run; `apply` and `features`
/// are the repo's live values at the start of the run.
pub struct RunContext {
    pub config: Arc<Config>,
    pub db: Arc<dyn DatabaseBackend>,
    pub gh: Arc<GhClient>,
    pub apply: bool,
    pub features: RepoFeatures,
    pub item: ItemRef,
    /// What started the run, e.g. `pull_request.synchronize`.
    pub trigger: String,
    /// Webhook event id, when the run comes from one.
    pub event_id: Option<i64>,
    item_cache: tokio::sync::Mutex<Option<ItemInfo>>,
    paths_cache: tokio::sync::Mutex<Option<Vec<String>>>,
}

impl RunContext {
    pub fn new(
        config: Arc<Config>,
        db: Arc<dyn DatabaseBackend>,
        gh: Arc<GhClient>,
        apply: bool,
        features: RepoFeatures,
        item: ItemRef,
        trigger: impl Into<String>,
    ) -> Self {
        Self {
            config,
            db,
            gh,
            apply,
            features,
            item,
            trigger: trigger.into(),
            event_id: None,
            item_cache: tokio::sync::Mutex::new(None),
            paths_cache: tokio::sync::Mutex::new(None),
        }
    }

    pub fn with_event_id(mut self, id: i64) -> Self {
        self.event_id = Some(id);
        self
    }

    fn info_from_db(&self) -> Option<ItemInfo> {
        let n = self.item.number?;
        match self.item.kind {
            ItemKind::Issue => self.db.get_issue(n).ok().flatten().map(|i| ItemInfo {
                labels: i.labels,
                author: i.author,
                head_sha: None,
            }),
            ItemKind::Pr => self.db.get_pull(n).ok().flatten().map(|p| ItemInfo {
                labels: p.labels,
                author: p.author,
                head_sha: p.head_sha,
            }),
        }
    }

    /// Labels / author of the item: the DB copy (fresh once a sync step
    /// ran), else one direct forge fetch, cached for the run. Never written
    /// to the DB here.
    pub async fn item_info(&self) -> Option<ItemInfo> {
        if let Some(i) = self.info_from_db() {
            return Some(i);
        }
        let mut cache = self.item_cache.lock().await;
        if cache.is_none() {
            let n = self.item.number?;
            let fetched = match self.item.kind {
                ItemKind::Issue => self
                    .gh
                    .fetch_issue(n)
                    .await
                    .ok()
                    .flatten()
                    .map(|i| ItemInfo {
                        labels: i.labels,
                        author: i.author,
                        head_sha: None,
                    }),
                ItemKind::Pr => self
                    .gh
                    .fetch_pull(n)
                    .await
                    .ok()
                    .flatten()
                    .map(|p| ItemInfo {
                        labels: p.labels,
                        author: p.author,
                        head_sha: p.head_sha,
                    }),
            };
            *cache = fetched;
        }
        cache.clone()
    }

    /// Changed paths of the PR (from its diff), fetched once per run.
    pub async fn changed_paths(&self) -> Result<Vec<String>, String> {
        let mut cache = self.paths_cache.lock().await;
        if let Some(p) = cache.as_ref() {
            return Ok(p.clone());
        }
        let n = self.item.number.ok_or("no PR number")?;
        let diff = self
            .gh
            .fetch_pr_diff(n)
            .await
            .map_err(|e| format!("could not fetch the PR diff: {e}"))?;
        let paths = condition::diff_paths(&diff);
        *cache = Some(paths.clone());
        Ok(paths)
    }
}

// ── AI call accounting ──────────────────────────────────────────────

tokio::task_local! {
    static AI_CALLS: Arc<AtomicU32>;
}

/// Count one AI call against the step running in the current task, if any.
/// Called by `AiBackend::complete`; a no-op outside a pipeline step (CLI,
/// scheduler, background diagram jobs).
pub fn note_ai_call() {
    let _ = AI_CALLS.try_with(|c| c.fetch_add(1, Ordering::Relaxed));
}

// ── Execution ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Done,
    Skipped,
    Failed,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StepStatus::Done => "done",
            StepStatus::Skipped => "skipped",
            StepStatus::Failed => "failed",
        }
    }
}

/// Outcome of one step of a run.
#[derive(Debug, Clone)]
pub struct StepRecord {
    pub step_id: String,
    pub uses: String,
    pub status: StepStatus,
    /// Skip reason, or error message of a failure.
    pub reason: Option<String>,
    pub summary: Option<String>,
    pub findings: u32,
    pub ai_calls: u32,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub duration_ms: u64,
}

/// Outcome of a run.
pub struct RunReport {
    pub template: String,
    pub run_id: Option<i64>,
    pub steps: Vec<StepRecord>,
    /// The aborting failure (step id, error), if any.
    pub failure: Option<(String, anyhow::Error)>,
}

impl RunReport {
    pub fn ai_calls(&self) -> u32 {
        self.steps.iter().map(|s| s.ai_calls).sum()
    }

    pub fn status(&self, step_id: &str) -> Option<StepStatus> {
        self.steps
            .iter()
            .find(|s| s.step_id == step_id)
            .map(|s| s.status)
    }

    /// `Err` with the aborting step's error, for the webhook event status.
    pub fn into_result(self) -> anyhow::Result<()> {
        match self.failure {
            Some((id, e)) => Err(e.context(format!("pipeline step '{id}' failed"))),
            None => Ok(()),
        }
    }
}

fn clip(s: &str) -> String {
    crate::pipelines::truncate(&crate::pro_hooks::apply_output_hook(s), MAX_TEXT)
}

/// Writes the run to the DB. Recording errors are logged and never fail
/// the run.
struct Recorder<'a> {
    db: &'a dyn DatabaseBackend,
    run_id: Option<i64>,
}

impl<'a> Recorder<'a> {
    fn start(ctx: &'a RunContext, template: &PipelineTemplate, plan: &[&StepDef]) -> Self {
        let Some(number) = ctx.item.number else {
            return Self {
                db: ctx.db.as_ref(),
                run_id: None,
            };
        };
        let planned: Vec<PlannedStep> = plan
            .iter()
            .map(|s| PlannedStep {
                id: s.id.clone(),
                uses: s.uses.clone(),
                needs: s.needs.clone(),
            })
            .collect();
        let run = NewPipelineRun {
            item_kind: ctx.item.kind.as_str().to_string(),
            item_number: number,
            template: template.name.clone(),
            trigger: ctx.trigger.clone(),
            event_id: ctx.event_id,
            head_sha: ctx.info_from_db().and_then(|i| i.head_sha),
            plan_json: serde_json::to_string(&planned).unwrap_or_else(|_| "[]".into()),
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        let run_id = match ctx.db.start_pipeline_run(&run) {
            Ok(id) => id,
            Err(e) => {
                warn!("Could not record pipeline run for {}: {e:#}", ctx.item);
                None
            }
        };
        Self {
            db: ctx.db.as_ref(),
            run_id,
        }
    }

    fn step(&self, seq: usize, r: &StepRecord) {
        let Some(run_id) = self.run_id else { return };
        let finished = r.started_at + chrono::Duration::milliseconds(r.duration_ms as i64);
        let row = PipelineStepRunRow {
            run_id,
            seq: seq as u32,
            step_id: r.step_id.clone(),
            uses: r.uses.clone(),
            status: r.status.as_str().to_string(),
            reason: r.reason.clone(),
            summary: r.summary.clone(),
            findings: r.findings,
            ai_calls: r.ai_calls,
            started_at: r.started_at.to_rfc3339(),
            finished_at: finished.to_rfc3339(),
            duration_ms: r.duration_ms,
        };
        if let Err(e) = self.db.record_pipeline_step(&row) {
            warn!("Could not record pipeline step '{}': {e:#}", r.step_id);
        }
    }

    fn finish(&self, ctx: &RunContext, report: &RunReport, started: Instant) {
        let Some(id) = self.run_id else { return };
        let fin = PipelineRunFinish {
            id,
            status: if report.failure.is_some() {
                "failed"
            } else {
                "done"
            }
            .to_string(),
            error: report
                .failure
                .as_ref()
                .map(|(s, e)| clip(&format!("{s}: {e:#}"))),
            head_sha: ctx.info_from_db().and_then(|i| i.head_sha),
            ai_calls: report.ai_calls(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            duration_ms: started.elapsed().as_millis() as u64,
        };
        if let Err(e) = self.db.finish_pipeline_run(&fin) {
            warn!("Could not finish pipeline run {id}: {e:#}");
        }
    }
}

/// Run `template` for the context's item and record it. Steps of the other
/// item kind are left out; the others are each done, skipped (with the
/// reason) or failed.
pub async fn execute(
    template: &PipelineTemplate,
    registry: &StepRegistry,
    ctx: &RunContext,
) -> RunReport {
    let plan = template::plan(template, ctx.item.kind, registry);
    let in_plan: std::collections::HashSet<&str> = plan.iter().map(|s| s.id.as_str()).collect();
    let recorder = Recorder::start(ctx, template, &plan);
    let started = Instant::now();
    let mut report = RunReport {
        template: template.name.clone(),
        run_id: recorder.run_id,
        steps: Vec::with_capacity(plan.len()),
        failure: None,
    };
    let mut statuses: HashMap<&str, StepStatus> = HashMap::new();
    let mut ai_used: u32 = 0;

    for (seq, def) in plan.iter().enumerate() {
        let step_started = chrono::Utc::now();
        let t0 = Instant::now();
        let mut rec = StepRecord {
            step_id: def.id.clone(),
            uses: def.uses.clone(),
            status: StepStatus::Skipped,
            reason: None,
            summary: None,
            findings: 0,
            ai_calls: 0,
            started_at: step_started,
            duration_ms: 0,
        };

        let gate = gate(def, ctx, &report, &statuses, &in_plan, template, ai_used).await;
        match gate {
            Err(reason) => {
                rec.reason = Some(clip(&reason));
            }
            Ok(()) => {
                let counter = Arc::new(AtomicU32::new(0));
                let outcome = run_step(def, registry, ctx, Arc::clone(&counter)).await;
                rec.ai_calls = counter.load(Ordering::Relaxed);
                ai_used = ai_used.saturating_add(rec.ai_calls);
                match outcome {
                    Ok(StepOutput {
                        skipped: Some(reason),
                        summary,
                        findings,
                    }) => {
                        rec.reason = Some(clip(&reason));
                        rec.summary = summary.as_deref().map(clip);
                        rec.findings = findings;
                    }
                    Ok(out) => {
                        rec.status = StepStatus::Done;
                        rec.summary = out.summary.as_deref().map(clip);
                        rec.findings = out.findings;
                    }
                    Err(e) => {
                        rec.status = StepStatus::Failed;
                        rec.reason = Some(clip(&format!("{e:#}")));
                        match def.on_failure {
                            OnFailure::Continue => {
                                warn!(
                                    "[{}] pipeline step '{}' failed for {} (continuing): {e:#}",
                                    ctx.config.repo_slug(),
                                    def.id,
                                    ctx.item
                                );
                            }
                            OnFailure::Abort => report.failure = Some((def.id.clone(), e)),
                        }
                    }
                }
            }
        }
        rec.duration_ms = t0.elapsed().as_millis() as u64;
        if rec.status == StepStatus::Skipped {
            info!(
                "[{}] {} step '{}' skipped: {}",
                ctx.config.repo_slug(),
                ctx.item,
                def.id,
                rec.reason.as_deref().unwrap_or("")
            );
        }
        statuses.insert(def.id.as_str(), rec.status);
        recorder.step(seq, &rec);
        report.steps.push(rec);
    }

    recorder.finish(ctx, &report, started);
    report
}

/// Why a step must not run, if it must not: an earlier abort, unmet
/// `needs`, the AI budget, or its conditions.
async fn gate(
    def: &StepDef,
    ctx: &RunContext,
    report: &RunReport,
    statuses: &HashMap<&str, StepStatus>,
    in_plan: &std::collections::HashSet<&str>,
    template: &PipelineTemplate,
    ai_used: u32,
) -> Result<(), String> {
    if let Some((failed, _)) = &report.failure {
        return Err(format!("run aborted: step '{failed}' failed"));
    }
    for n in &def.needs {
        match statuses.get(n.as_str()) {
            Some(StepStatus::Done) => {}
            Some(s) => return Err(format!("needs '{n}', which was {}", s.as_str())),
            None if !in_plan.contains(n.as_str()) => {
                return Err(format!("needs '{n}', which does not run on {}", ctx.item))
            }
            None => return Err(format!("needs '{n}', which did not run")),
        }
    }
    if let Some(budget) = template.max_ai_calls {
        if ai_used >= budget {
            return Err(format!("AI call budget of {budget} reached"));
        }
    }
    condition::evaluate(&def.when, ctx).await
}

async fn run_step(
    def: &StepDef,
    registry: &StepRegistry,
    ctx: &RunContext,
    counter: Arc<AtomicU32>,
) -> anyhow::Result<StepOutput> {
    let Some(step) = registry.get(&def.uses) else {
        anyhow::bail!("unknown step kind '{}'", def.uses);
    };
    let fut = AI_CALLS.scope(counter, step.run(ctx, def));
    match def.timeout_secs {
        Some(secs) => tokio::time::timeout(std::time::Duration::from_secs(secs), fut)
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("timed out after {secs}s"))),
        None => fut.await,
    }
}

/// Select the template for the context's item (routes, then `standard`),
/// with the repo's effective config (TOML + DB override). A template whose
/// steps do not validate against `registry` is replaced by `standard`.
pub async fn select_template(ctx: &RunContext, registry: &StepRegistry) -> PipelineTemplate {
    let cfg = template::load(&ctx.config, ctx.db.as_ref());
    let name = template::select(&cfg, |c| async move {
        condition::evaluate(c, ctx).await.is_ok()
    })
    .await
    .to_string();
    let t = match template::find(&cfg, &name) {
        Some(t) => t.clone(),
        None => template::standard().clone(),
    };
    if let Err(e) = template::validate_template_steps(&t, registry) {
        error!(
            "[{}] pipeline '{name}' is invalid, running '{}' instead: {e}",
            ctx.config.repo_slug(),
            template::STANDARD
        );
        return template::standard().clone();
    }
    t
}

/// Entry point of the event processor: pick the template, run it, record
/// it. `Err` when a step with `on_failure = "abort"` failed.
pub async fn run_item(ctx: &RunContext) -> anyhow::Result<()> {
    let registry = StepRegistry::global();
    let template = select_template(ctx, &registry).await;
    info!(
        "[{}] {} → pipeline '{}' ({})",
        ctx.config.repo_slug(),
        ctx.item,
        template.name,
        ctx.trigger
    );
    execute(&template, &registry, ctx).await.into_result()
}

/// Apply the run retention of the repo's effective config.
pub fn cleanup_runs(config: &Config, db: &dyn DatabaseBackend) -> anyhow::Result<u64> {
    let r = template::load(config, db).pipeline_runs;
    db.cleanup_pipeline_runs(r.retention_days, r.max_per_item)
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub fn ctx_with(kind: ItemKind, number: Option<u64>, features: RepoFeatures) -> RunContext {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let config = Config {
            repo_owner: "o".into(),
            repo_name: "r".into(),
            ..Default::default()
        };
        let db: Arc<dyn DatabaseBackend> = Arc::new(crate::db::Database::open_memory().unwrap());
        // One provider for every test context: building it may shell out
        // to `gh auth token`, which is slow. No test reaches the network.
        static GH: std::sync::OnceLock<Arc<GhClient>> = std::sync::OnceLock::new();
        let gh =
            Arc::clone(GH.get_or_init(|| {
                crate::git_provider::build_provider_with_db(&config, None).unwrap()
            }));
        RunContext::new(
            Arc::new(config),
            db,
            gh,
            false,
            features,
            ItemRef { kind, number },
            "test",
        )
    }

    pub fn seed_issue(ctx: &RunContext, n: u64, labels: &[&str], author: &str) {
        ctx.db
            .upsert_issue(&crate::db::issues::Issue {
                number: n,
                title: "t".into(),
                body: None,
                state: "open".into(),
                labels: labels.iter().map(|s| s.to_string()).collect(),
                author: Some(author.into()),
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
                reactions_plus1: 0,
                reactions_total: 0,
            })
            .unwrap();
    }

    pub fn seed_pull(ctx: &RunContext, n: u64, author: &str, base: &str) {
        ctx.db
            .upsert_pull(&crate::db::pulls::PullRequest {
                number: n,
                title: "t".into(),
                body: None,
                state: "open".into(),
                labels: Vec::new(),
                author: Some(author.into()),
                head_sha: Some("abc123".into()),
                base_sha: None,
                head_ref: None,
                base_ref: Some(base.into()),
                mergeable: None,
                ci_status: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
                review_decision: None,
                review_decision_at: None,
                reactions_plus1: 0,
            })
            .unwrap();
    }
}

#[cfg(test)]
mod tests;
