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

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemInfo {
    pub labels: Vec<String>,
    pub author: Option<String>,
    pub head_sha: Option<String>,
}

impl ItemInfo {
    /// Labels / author / head of the item as the webhook payload carries
    /// them (`pull_request` or `issue` object). `None` when the payload has
    /// no label list (the poller's synthesized events).
    pub fn from_payload(kind: ItemKind, payload: &serde_json::Value) -> Option<Self> {
        let obj = match kind {
            ItemKind::Pr => payload
                .get("pull_request")
                .filter(|v| v.get("labels").is_some())
                .or_else(|| payload.get("issue")),
            ItemKind::Issue => payload.get("issue"),
        }?;
        let labels = obj
            .get("labels")?
            .as_array()?
            .iter()
            .filter_map(|l| {
                l.get("name")
                    .and_then(|n| n.as_str())
                    .or_else(|| l.as_str())
                    .map(str::to_string)
            })
            .collect();
        Some(Self {
            labels,
            author: obj
                .get("user")
                .and_then(|u| u.get("login"))
                .and_then(|l| l.as_str())
                .map(str::to_string),
            head_sha: obj
                .get("head")
                .and_then(|h| h.get("sha"))
                .and_then(|s| s.as_str())
                .map(str::to_string),
        })
    }
}

/// What started a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunSource {
    /// A webhook (or polled) issue / PR event: a full run.
    #[default]
    Event,
    /// The scheduler's periodic triage: a partial run of the `triage` steps.
    Scheduler,
    /// A slash command: a partial run of the steps of the command's kind.
    Command,
}

/// Receives the AI calls made by work a step left running in the
/// background (diagram jobs), to add them to the run.
pub type AiCallSink = Box<dyn FnOnce(u32) + Send + 'static>;

#[derive(Debug, Default)]
struct Progress {
    run_id: Option<i64>,
    seq: u32,
    ai_used: u32,
    budget: Option<u32>,
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
    /// What started the run, e.g. `pull_request.synchronize`, `scheduler`,
    /// `command`.
    pub trigger: String,
    pub source: RunSource,
    /// Webhook event id, when the run comes from one.
    pub event_id: Option<i64>,
    /// Item fields of the webhook payload. Used to pick the template
    /// (routes run before any sync, when the cache can be stale, e.g. a
    /// label just added by `pull_request.labeled`); steps then read the DB.
    payload_info: Option<ItemInfo>,
    steps_started: AtomicBool,
    /// Forge fetch of the item, done at most once per run (even when it
    /// fails or finds nothing).
    item_cache: tokio::sync::Mutex<Option<Option<ItemInfo>>>,
    paths_cache: tokio::sync::Mutex<Option<Vec<String>>>,
    progress: std::sync::Mutex<Progress>,
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
            source: RunSource::Event,
            event_id: None,
            payload_info: None,
            steps_started: AtomicBool::new(false),
            item_cache: tokio::sync::Mutex::new(None),
            paths_cache: tokio::sync::Mutex::new(None),
            progress: std::sync::Mutex::new(Progress::default()),
        }
    }

    pub fn with_event_id(mut self, id: i64) -> Self {
        self.event_id = Some(id);
        self
    }

    pub fn with_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.trigger = trigger.into();
        self
    }

    pub fn with_source(mut self, source: RunSource) -> Self {
        self.source = source;
        self
    }

    pub fn with_payload_info(mut self, info: Option<ItemInfo>) -> Self {
        self.payload_info = info;
        self
    }

    fn progress(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(|p| p.into_inner())
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

    /// Labels / author of the item. Before the first step (template
    /// selection): the webhook payload when it has them. Then: the DB copy
    /// (fresh once a sync step ran), else one direct forge fetch per run,
    /// whose result is cached even when it fails. Never written to the DB.
    pub async fn item_info(&self) -> Option<ItemInfo> {
        if !self.steps_started.load(Ordering::Relaxed) {
            if let Some(p) = &self.payload_info {
                return Some(p.clone());
            }
        }
        if let Some(i) = self.info_from_db() {
            return Some(i);
        }
        let n = self.item.number?;
        let mut cache = self.item_cache.lock().await;
        if cache.is_none() {
            let fetched = match self.item.kind {
                ItemKind::Issue => match self.gh.fetch_issue(n).await {
                    Ok(i) => i.map(|i| ItemInfo {
                        labels: i.labels,
                        author: i.author,
                        head_sha: None,
                    }),
                    Err(e) => {
                        warn!(
                            "Could not fetch {} for the pipeline conditions: {e:#}",
                            self.item
                        );
                        None
                    }
                },
                ItemKind::Pr => match self.gh.fetch_pull(n).await {
                    Ok(p) => p.map(|p| ItemInfo {
                        labels: p.labels,
                        author: p.author,
                        head_sha: p.head_sha,
                    }),
                    Err(e) => {
                        warn!(
                            "Could not fetch {} for the pipeline conditions: {e:#}",
                            self.item
                        );
                        None
                    }
                },
            };
            *cache = Some(fetched);
        }
        cache.clone().flatten()
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

    /// AI calls left in the run's budget, counting the calls of the
    /// running step so far. `None` = no budget.
    pub fn budget_left(&self) -> Option<u32> {
        let current = AI_CALLS
            .try_with(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0);
        let p = self.progress();
        p.budget
            .map(|b| b.saturating_sub(p.ai_used.saturating_add(current)))
    }

    /// A sink adding background AI calls to the running step and its run.
    /// `None` when the run is not recorded.
    pub fn ai_call_sink(&self) -> Option<AiCallSink> {
        let (run_id, seq) = {
            let p = self.progress();
            (p.run_id?, p.seq)
        };
        let db = Arc::clone(&self.db);
        Some(Box::new(move |n: u32| {
            if n > 0 {
                if let Err(e) = db.add_pipeline_ai_calls(run_id, seq, n) {
                    warn!("Could not add {n} background AI call(s) to run {run_id}: {e:#}");
                }
            }
        }))
    }
}

// ── AI call accounting ──────────────────────────────────────────────

tokio::task_local! {
    static AI_CALLS: Arc<AtomicU32>;
}

/// Count one AI call against the step running in the current task, if any.
/// Called by `AiBackend::complete`; a no-op outside a pipeline step (CLI)
/// or a [`count_ai_calls`] scope.
pub fn note_ai_call() {
    let _ = AI_CALLS.try_with(|c| c.fetch_add(1, Ordering::Relaxed));
}

/// Run `fut` counting the AI calls it makes (for work spawned outside the
/// step's task, e.g. diagram jobs).
pub async fn count_ai_calls<F: std::future::Future>(fut: F) -> (F::Output, u32) {
    let counter = Arc::new(AtomicU32::new(0));
    let out = AI_CALLS.scope(Arc::clone(&counter), fut).await;
    (out, counter.load(Ordering::Relaxed))
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

/// How [`execute_with`] runs a template.
#[derive(Debug, Clone, Default)]
pub struct ExecOptions {
    /// Partial run: only the steps of these kinds. Their `needs` on steps
    /// left out are treated as met (the caller did that work, e.g. the
    /// scheduler synced already).
    pub only_kinds: Option<Vec<String>>,
    /// Ignore the repo-toggle conditions (`features`, `repo_filters`): the
    /// toggles govern automatic processing, not explicit slash commands.
    pub ignore_toggles: bool,
    /// Record the run only once a step passes its gate (the scheduler
    /// would otherwise record an all-skipped run per issue and cycle).
    pub record_only_if_active: bool,
}

fn clip(s: &str) -> String {
    crate::pipelines::truncate(&crate::pro_hooks::apply_output_hook(s), MAX_TEXT)
}

/// Writes the run to the DB, at once or lazily (see
/// [`ExecOptions::record_only_if_active`]). Recording errors are logged and
/// never fail the run.
struct Recorder<'a> {
    ctx: &'a RunContext,
    new_run: Option<NewPipelineRun>,
    run_id: Option<i64>,
    pending: Vec<(usize, StepRecord)>,
}

impl<'a> Recorder<'a> {
    fn new(ctx: &'a RunContext, template: &PipelineTemplate, plan: &[&StepDef]) -> Self {
        let new_run = ctx.item.number.map(|number| {
            let planned: Vec<PlannedStep> = plan
                .iter()
                .map(|s| PlannedStep {
                    id: s.id.clone(),
                    uses: s.uses.clone(),
                    needs: s.needs.clone(),
                })
                .collect();
            NewPipelineRun {
                item_kind: ctx.item.kind.as_str().to_string(),
                item_number: number,
                template: template.name.clone(),
                trigger: ctx.trigger.clone(),
                event_id: ctx.event_id,
                head_sha: ctx.info_from_db().and_then(|i| i.head_sha),
                plan_json: serde_json::to_string(&planned).unwrap_or_else(|_| "[]".into()),
                started_at: chrono::Utc::now().to_rfc3339(),
            }
        });
        Self {
            ctx,
            new_run,
            run_id: None,
            pending: Vec::new(),
        }
    }

    /// Insert the run (once) and flush the steps recorded so far.
    fn start(&mut self) {
        let Some(run) = self.new_run.take() else {
            return;
        };
        self.run_id = match self.ctx.db.start_pipeline_run(&run) {
            Ok(id) => id,
            Err(e) => {
                warn!("Could not record pipeline run for {}: {e:#}", self.ctx.item);
                None
            }
        };
        self.ctx.progress().run_id = self.run_id;
        for (seq, r) in std::mem::take(&mut self.pending) {
            self.write_step(seq, &r);
        }
    }

    fn started(&self) -> bool {
        self.new_run.is_none()
    }

    fn step(&mut self, seq: usize, r: &StepRecord) {
        if self.started() {
            self.write_step(seq, r);
        } else {
            self.pending.push((seq, r.clone()));
        }
    }

    fn write_step(&self, seq: usize, r: &StepRecord) {
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
        if let Err(e) = self.ctx.db.record_pipeline_step(&row) {
            warn!("Could not record pipeline step '{}': {e:#}", r.step_id);
        }
    }

    fn finish(&self, report: &RunReport, started: Instant) {
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
            head_sha: self.ctx.info_from_db().and_then(|i| i.head_sha),
            ai_calls: report.ai_calls(),
            finished_at: chrono::Utc::now().to_rfc3339(),
            duration_ms: started.elapsed().as_millis() as u64,
        };
        if let Err(e) = self.ctx.db.finish_pipeline_run(&fin) {
            warn!("Could not finish pipeline run {id}: {e:#}");
        }
    }
}

/// Run `template` for the context's item and record it (full run).
pub async fn execute(
    template: &PipelineTemplate,
    registry: &StepRegistry,
    ctx: &RunContext,
) -> RunReport {
    execute_with(template, registry, ctx, &ExecOptions::default()).await
}

/// Run `template` for the context's item and record it. Steps of the other
/// item kind (and, for a partial run, of other kinds) are left out; the
/// others are each done, skipped (with the reason) or failed.
pub async fn execute_with(
    template: &PipelineTemplate,
    registry: &StepRegistry,
    ctx: &RunContext,
    opts: &ExecOptions,
) -> RunReport {
    let mut plan = template::plan(template, ctx.item.kind, registry);
    let in_template: HashSet<&str> = plan.iter().map(|s| s.id.as_str()).collect();
    if let Some(kinds) = &opts.only_kinds {
        plan.retain(|s| kinds.contains(&s.uses));
    }
    let in_plan: HashSet<&str> = plan.iter().map(|s| s.id.as_str()).collect();
    let mut recorder = Recorder::new(ctx, template, &plan);
    if !opts.record_only_if_active {
        recorder.start();
    }
    ctx.steps_started.store(true, Ordering::Relaxed);
    ctx.progress().budget = template.max_ai_calls;
    let started = Instant::now();
    let mut report = RunReport {
        template: template.name.clone(),
        run_id: None,
        steps: Vec::with_capacity(plan.len()),
        failure: None,
    };
    let mut statuses: HashMap<&str, StepStatus> = HashMap::new();

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

        let scope = Scope {
            in_plan: &in_plan,
            in_template: &in_template,
            partial: opts.only_kinds.is_some(),
            ignore_toggles: opts.ignore_toggles,
        };
        match gate(def, ctx, &report, &statuses, &scope, template).await {
            Err(reason) => {
                rec.reason = Some(clip(&reason));
            }
            Ok(()) => {
                recorder.start();
                ctx.progress().seq = seq as u32;
                let counter = Arc::new(AtomicU32::new(0));
                let outcome = run_step(def, registry, ctx, Arc::clone(&counter)).await;
                rec.ai_calls = counter.load(Ordering::Relaxed);
                {
                    let mut p = ctx.progress();
                    p.ai_used = p.ai_used.saturating_add(rec.ai_calls);
                }
                match outcome {
                    Ok(out) => {
                        rec.summary = out.summary.as_deref().map(clip);
                        rec.findings = out.findings;
                        if let Some(e) = out.failed {
                            rec.status = StepStatus::Failed;
                            rec.reason = Some(clip(&e));
                        } else if let Some(reason) = out.skipped {
                            rec.reason = Some(clip(&reason));
                        } else {
                            rec.status = StepStatus::Done;
                        }
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

    report.run_id = recorder.run_id;
    recorder.finish(&report, started);
    report
}

struct Scope<'a> {
    in_plan: &'a HashSet<&'a str>,
    in_template: &'a HashSet<&'a str>,
    partial: bool,
    ignore_toggles: bool,
}

/// Why a step must not run, if it must not: an earlier abort, unmet
/// `needs`, the AI budget, or its conditions.
async fn gate(
    def: &StepDef,
    ctx: &RunContext,
    report: &RunReport,
    statuses: &HashMap<&str, StepStatus>,
    scope: &Scope<'_>,
    template: &PipelineTemplate,
) -> Result<(), String> {
    if let Some((failed, _)) = &report.failure {
        return Err(format!("run aborted: step '{failed}' failed"));
    }
    for n in &def.needs {
        match statuses.get(n.as_str()) {
            Some(StepStatus::Done) => {}
            Some(s) => return Err(format!("needs '{n}', which was {}", s.as_str())),
            None if scope.partial
                && !scope.in_plan.contains(n.as_str())
                && scope.in_template.contains(n.as_str()) => {}
            None if !scope.in_template.contains(n.as_str()) => {
                return Err(format!("needs '{n}', which does not run on {}", ctx.item))
            }
            None => return Err(format!("needs '{n}', which did not run")),
        }
    }
    if let Some(budget) = template.max_ai_calls {
        if ctx.progress().ai_used >= budget {
            return Err(format!("AI call budget of {budget} reached"));
        }
    }
    if scope.ignore_toggles {
        let mut c = def.when.clone();
        c.features.clear();
        c.repo_filters = false;
        condition::evaluate(&c, ctx).await
    } else {
        condition::evaluate(&def.when, ctx).await
    }
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
        // Dropping the future stops the step wherever it is: what it did
        // before (labels, comments) stays. See docs/configuration.md.
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

/// Partial run of the item's template: only the steps of `kinds` (see
/// [`ExecOptions`]). Used by the scheduler and the slash commands so they
/// follow the template (and its step conditions) instead of calling the
/// pipelines directly.
pub async fn run_kinds(ctx: &RunContext, kinds: &[&str], mut opts: ExecOptions) -> RunReport {
    let registry = StepRegistry::global();
    let template = select_template(ctx, &registry).await;
    opts.only_kinds = Some(kinds.iter().map(|k| k.to_string()).collect());
    execute_with(&template, &registry, ctx, &opts).await
}

/// Log, once per repo at boot, what is wrong with its pipeline config:
/// invalid TOML (the repo runs `standard`), dropped DB override parts,
/// templates with unknown step kinds, and warnings.
pub fn log_config_problems(config: &Config, db: &dyn DatabaseBackend) {
    let slug = config.repo_slug();
    let loaded = template::load_full(config, db);
    if let Some(e) = &loaded.toml_error {
        error!(
            "[{slug}] invalid pipeline config in TOML, running '{}': {e}",
            template::STANDARD
        );
    }
    for e in &loaded.override_errors {
        warn!("[{slug}] pipelines DB override: {e}");
    }
    let registry = StepRegistry::global();
    for t in &loaded.config.pipelines {
        if let Err(e) = template::validate_template_steps(t, &registry) {
            error!(
                "[{slug}] {e} (runs routed to it use '{}')",
                template::STANDARD
            );
        }
    }
    for w in template::warnings(&loaded.config, &registry) {
        warn!("[{slug}] {w}");
    }
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
