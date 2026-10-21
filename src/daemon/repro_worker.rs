//! Background worker for automatic issue reproduction (#229).
//!
//! Triage pushes `(repo, issue, category)` into a bounded queue with
//! `pipelines::repro::notify_triaged` (non-blocking). This worker applies the
//! gating (`repro_issues` switch, apply mode, category, sandbox registered)
//! and runs at most [`MAX_CONCURRENT_REPRO`] jobs at once with its own
//! semaphore, so reproduction never holds the event-processor permits nor
//! delays triage. When the queue is full new requests are dropped (logged).

use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{mpsc, Semaphore};
use tracing::{debug, error, info, warn};

use super::{DaemonState, MultiDaemonState};
use crate::pipelines::repro::{self, ReproConfig, ReproRequest};
use crate::pro_hooks;

pub const MAX_CONCURRENT_REPRO: usize = 2;
const QUEUE_CAPACITY: usize = 64;

pub enum Resolver {
    Single(Arc<DaemonState>),
    Multi(Arc<MultiDaemonState>),
}

impl Resolver {
    async fn get(&self, slug: &str) -> Option<Arc<DaemonState>> {
        match self {
            Resolver::Single(s) => (s.config.repo_slug() == slug).then(|| Arc::clone(s)),
            Resolver::Multi(m) => m.repos.read().await.get(slug).cloned(),
        }
    }
}

/// Install the queue and start the worker. No-op (returns `None`) when a
/// worker is already installed in this process.
pub fn spawn(resolver: Resolver) -> Option<tokio::task::JoinHandle<()>> {
    let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
    if !repro::install_queue(tx) {
        return None;
    }
    Some(tokio::spawn(run(resolver, rx)))
}

type InFlight = Arc<StdMutex<HashSet<(String, u64)>>>;

struct Guard(InFlight, (String, u64));

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(mut s) = self.0.lock() {
            s.remove(&self.1);
        }
    }
}

async fn run(resolver: Resolver, mut rx: mpsc::Receiver<ReproRequest>) {
    info!("Reproduction worker started (max {MAX_CONCURRENT_REPRO} concurrent)");
    if let Some(sb) = pro_hooks::repro_sandbox() {
        (sb.cleanup)().await;
    }
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_REPRO));
    let in_flight: InFlight = Arc::new(StdMutex::new(HashSet::new()));

    while let Some(req) = rx.recv().await {
        let Some(state) = resolver.get(&req.repo).await else {
            debug!("Repro: unknown repo '{}', dropping", req.repo);
            continue;
        };
        let cfg = ReproConfig::effective(&state.config.triage.repro, state.db.as_ref());
        let sandbox = pro_hooks::repro_sandbox();
        if let Err(skip) = repro::should_queue(
            &state.features(),
            state.apply(),
            &cfg,
            &req.category,
            sandbox.is_some(),
        ) {
            match skip {
                repro::SkipReason::NoSandbox => {
                    warn!("[{}] Repro #{} skipped: {skip}", req.repo, req.issue_number)
                }
                _ => debug!("[{}] Repro #{} skipped: {skip}", req.repo, req.issue_number),
            }
            continue;
        }
        let Some(sandbox) = sandbox else { continue };

        let key = (req.repo.clone(), req.issue_number);
        let inserted = match in_flight.lock() {
            Ok(mut s) => s.insert(key.clone()),
            Err(_) => false, // poisoned: fail closed
        };
        if !inserted {
            debug!("Repro #{} already in flight", req.issue_number);
            continue;
        }
        let guard = Guard(Arc::clone(&in_flight), key);

        // Bounded: wait here (the queue absorbs bursts) rather than
        // spawning an unbounded number of waiting tasks.
        let Ok(permit) = Arc::clone(&sem).acquire_owned().await else {
            break;
        };
        tokio::spawn(async move {
            let _permit = permit;
            let _guard = guard;
            // Hard ceiling on one job (clone + AI + sandbox + publish).
            let budget = Duration::from_secs(cfg.timeout_secs + 900);
            match tokio::time::timeout(budget, run_one(&state, &req, cfg, sandbox)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => error!("Repro #{} failed: {e:#}", req.issue_number),
                Err(_) => error!("Repro #{} exceeded its time budget", req.issue_number),
            }
        });
    }
    info!("Reproduction worker stopped");
}

async fn run_one(
    state: &DaemonState,
    req: &ReproRequest,
    cfg: ReproConfig,
    sandbox: pro_hooks::ReproSandbox,
) -> anyhow::Result<()> {
    let Some(issue) = state.db.get_issue(req.issue_number)? else {
        anyhow::bail!("issue not in cache");
    };
    let config = &state.config;
    let Some(clone_url) = crate::git_provider::clone_url(config) else {
        anyhow::bail!("cannot build a clone URL for this forge");
    };
    let backend = crate::ai::backend::AiBackend::from_config(config, config.model_for("triage"))?;
    let planner = repro::AiPlanner(backend);
    let gh = state.gh();
    let ctx = repro::JobContext {
        config,
        db: state.db.as_ref(),
        gh: Some(&*gh),
        cfg,
        // Global cap: TOML value only (not overridable per repo).
        global_max: config.triage.repro.max_runs_per_day_global,
        write: state.apply(),
        clone_url,
        git_env: repro::git_auth_env(config),
        work_root: config.wshm_dir.join("repro-work"),
    };
    repro::run_job(&ctx, &issue, &planner, &sandbox).await?;
    Ok(())
}
