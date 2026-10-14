pub mod commands;
pub mod log_buffer;
pub mod memory;
pub mod poller;
pub mod processor;
pub mod scheduler;
pub mod server;
pub mod systemd;
pub mod web;

/// Maximum number of webhook events buffered in memory before backpressure.
const WEBHOOK_CHANNEL_CAPACITY: usize = 256;

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn};

use crate::cli::DaemonArgs;
use crate::config::{Config, GlobalConfig};
use crate::db::backend::DatabaseBackend;
use crate::db::Database;
use crate::github::Client as GhClient;

use self::processor::WebhookEvent;

pub struct DaemonState {
    pub db: Arc<dyn DatabaseBackend>,
    /// Hot-reloadable GitHub client. The inner Arc is swapped under a
    /// RwLock when a `github_token` secret is added or removed via the
    /// web UI, so the new token is picked up without a daemon restart.
    /// Call sites get a snapshot via [`DaemonState::gh()`].
    gh: std::sync::RwLock<Arc<GhClient>>,
    pub config: Arc<Config>,
    /// Master apply mode for this repo: `true` = post comments / labels /
    /// PRs to GitHub, `false` = compute results visible in the dashboard
    /// only (DRY-RUN). Mutated at runtime via the Settings → Repos modal
    /// (PATCH /api/v1/repos/{slug}/features `{"apply": true|false}`); use
    /// the [`DaemonState::apply`] getter — pipelines must read the live
    /// value, not capture a startup snapshot.
    apply: std::sync::atomic::AtomicBool,
    /// Per-repo feature toggles. Snapshot via [`DaemonState::features`].
    /// Pipelines must check the relevant flag before performing mutating
    /// actions (triage, analyze, auto-fix, merge).
    features: std::sync::RwLock<crate::config::RepoFeatures>,
    /// Set when the repo is removed at runtime; the scheduler and poller
    /// check [`DaemonState::is_cancelled`] each cycle and exit so a removed
    /// repo stops writing to the DB without a daemon restart.
    cancelled: std::sync::atomic::AtomicBool,
    /// Wakes the scheduler/poller immediately on cancellation instead of
    /// waiting out the current sleep interval.
    cancel_notify: tokio::sync::Notify,
}

impl DaemonState {
    pub fn new(
        db: Arc<dyn DatabaseBackend>,
        gh: Arc<GhClient>,
        config: Arc<Config>,
        apply: bool,
    ) -> Self {
        // Legacy `apply: true` upgrades the triage/analyze/auto_pr trio so
        // existing setups keep behaving the same after this migration.
        let mut features = crate::config::RepoFeatures::default();
        features.merge_legacy_apply(apply);
        Self::with_features(db, gh, config, apply, features)
    }

    pub fn with_features(
        db: Arc<dyn DatabaseBackend>,
        gh: Arc<GhClient>,
        config: Arc<Config>,
        apply: bool,
        features: crate::config::RepoFeatures,
    ) -> Self {
        crate::ai::related::set_repo_enabled(&config.repo_slug(), features.related_history);
        // Every daemon path (single, multi-repo, runtime add_repo, Pro's
        // Postgres factory) builds its per-repo state here, so this is the one
        // place TOML `[[skills]]` get seeded into the shared DB list.
        crate::config::seed_skills_logged(db.as_ref(), &config.skills);
        Self {
            db,
            gh: std::sync::RwLock::new(gh),
            config,
            apply: std::sync::atomic::AtomicBool::new(apply),
            features: std::sync::RwLock::new(features),
            cancelled: std::sync::atomic::AtomicBool::new(false),
            cancel_notify: tokio::sync::Notify::new(),
        }
    }

    /// True once the repo has been removed at runtime. Long-running loops
    /// (scheduler, poller) must check this each cycle and exit.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Signal the scheduler/poller to stop and wake them immediately.
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.cancel_notify.notify_waiters();
    }

    /// Sleep for `dur`, returning early if the repo is cancelled. Returns
    /// `true` if cancelled (caller should break its loop).
    pub async fn sleep_or_cancel(&self, dur: std::time::Duration) -> bool {
        if self.is_cancelled() {
            return true;
        }
        tokio::select! {
            _ = tokio::time::sleep(dur) => {}
            _ = self.cancel_notify.notified() => {}
        }
        self.is_cancelled()
    }

    /// Live apply-mode flag. `true` → mutating actions hit GitHub;
    /// `false` → DRY-RUN (compute + log only).
    pub fn apply(&self) -> bool {
        self.apply.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Toggle apply mode at runtime. Persistence to global.toml is the
    /// caller's responsibility (see api_repo_features_patch).
    pub fn set_apply(&self, v: bool) {
        self.apply.store(v, std::sync::atomic::Ordering::Relaxed);
    }

    /// Snapshot of the current GitHub client. The returned Arc is
    /// independent of subsequent reloads, so an in-flight call keeps
    /// using the old client even if the secret changes mid-request.
    pub fn gh(&self) -> Arc<GhClient> {
        self.gh.read().expect("gh RwLock poisoned").clone()
    }

    /// Rebuild the GitHub client from the current config (re-reads the
    /// secret store) and atomically swap the inner Arc. Called after a
    /// `github_token` secret is added or removed via the web UI.
    pub fn reload_github_client(&self) -> anyhow::Result<()> {
        let new_client = GhClient::new(&self.config)?;
        let was_authenticated = self.gh().authenticated;
        let now_authenticated = new_client.authenticated;
        *self.gh.write().expect("gh RwLock poisoned") = Arc::new(new_client);
        info!(
            "[{}] GitHub client reloaded ({} → {})",
            self.config.repo_slug(),
            if was_authenticated {
                "authenticated"
            } else {
                "anonymous"
            },
            if now_authenticated {
                "authenticated"
            } else {
                "anonymous"
            }
        );
        Ok(())
    }

    /// Snapshot of the current per-repo feature flags.
    pub fn features(&self) -> crate::config::RepoFeatures {
        self.features
            .read()
            .expect("features RwLock poisoned")
            .clone()
    }

    /// Replace the in-memory feature flags. The API handler is responsible
    /// for also persisting them to global.toml so they survive restart.
    pub fn set_features(&self, new: crate::config::RepoFeatures) {
        crate::ai::related::set_repo_enabled(&self.config.repo_slug(), new.related_history);
        *self.features.write().expect("features RwLock poisoned") = new;
    }
}

/// Runtime context captured at daemon startup so dynamic add_repo can spawn
/// scheduler/poller for newly added repos without restart. Set only in
/// multi-repo mode (`run_multi`); None when the daemon is running mono-repo.
#[derive(Clone)]
pub struct DynamicRuntime {
    pub event_tx: mpsc::Sender<(String, WebhookEvent)>,
    pub poll: bool,
    pub poll_interval: u64,
    pub global_apply: bool,
    pub global_config_path: PathBuf,
    /// Canonical repo registry (list + per-repo settings). Source of truth for
    /// persistence; seeded from `GlobalConfig.repos` at boot and mutated by
    /// add/remove/feature edits.
    pub repos: Arc<RwLock<Vec<crate::config::RepoEntry>>>,
    /// Persist the registry (Pro → Postgres). When `None`, `persist()` writes
    /// `global.toml` at `global_config_path`.
    pub persist_repos: Option<RepoPersist>,
    /// Re-read the persisted registry (Pro → Postgres) so this pod picks up
    /// edits made through another pod. `None` (OSS: per-pod SQLite/TOML,
    /// nothing shared) disables [`MultiDaemonState::sync_registry`].
    pub load_repos: Option<RepoLoad>,
    /// Serialises every registry read-modify-write on this pod: the periodic
    /// sync (load + reconcile) and each handler's sync → mutate → persist.
    /// Without it a sync could interleave between a handler's mutation and
    /// its persist and revert the edit (in memory and in the store).
    pub registry_lock: Arc<tokio::sync::Mutex<()>>,
    /// Build a repo's storage backend (Pro → shared Postgres pool). When
    /// `None`, a runtime-added repo opens a per-repo SQLite `Database`.
    pub db_factory: Option<DbFactory>,
}

impl DynamicRuntime {
    /// Persist the current registry: via the Pro hook when set, else by
    /// rewriting `global.toml`. Best-effort; errors are returned to the caller.
    pub async fn persist(&self) -> Result<()> {
        let repos = self.repos.read().await.clone();
        if let Some(hook) = &self.persist_repos {
            hook(&repos)
        } else {
            let mut global = GlobalConfig::load(&self.global_config_path).unwrap_or_default();
            global.repos = repos;
            global.save(&self.global_config_path)
        }
    }
}

/// Multi-repo state: maps "owner/repo" slug to its DaemonState.
pub struct MultiDaemonState {
    pub repos: RwLock<HashMap<String, Arc<DaemonState>>>,
    pub runtime: Option<DynamicRuntime>,
}

impl MultiDaemonState {
    pub fn new(repos: HashMap<String, Arc<DaemonState>>) -> Self {
        Self {
            repos: RwLock::new(repos),
            runtime: None,
        }
    }

    pub fn with_runtime(repos: HashMap<String, Arc<DaemonState>>, runtime: DynamicRuntime) -> Self {
        Self {
            repos: RwLock::new(repos),
            runtime: Some(runtime),
        }
    }

    /// Pull the persisted registry and reconcile this pod's live state with
    /// it: repos added elsewhere are started, repos removed elsewhere are
    /// cancelled, and for repos already running only the fields whose stored
    /// value changed since this pod's previous snapshot are applied
    /// (features verbatim — the legacy `apply` upgrade is a boot-time rule and
    /// is never re-run here). An unchanged registry costs one read and touches
    /// nothing. Returns `true` when something was reconciled.
    ///
    /// Takes [`DynamicRuntime::registry_lock`], the same lock registry
    /// mutations (features PATCH, add/remove repo) hold for their whole
    /// sync → mutate → persist sequence, so a periodic sync can never slip
    /// between a handler's mutation and its persist.
    pub async fn sync_registry(&self) -> Result<bool> {
        let Some(runtime) = self.runtime.as_ref() else {
            return Ok(false);
        };
        let _guard = runtime.registry_lock.lock().await;
        self.sync_registry_locked().await
    }

    /// Acquire the registry lock for a sync → mutate → persist sequence.
    /// `None` outside multi-repo mode.
    pub async fn lock_registry(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        match self.runtime.as_ref() {
            Some(rt) => Some(rt.registry_lock.lock().await),
            None => None,
        }
    }

    /// [`Self::sync_registry`] body. The caller MUST hold the guard returned
    /// by [`Self::lock_registry`].
    pub async fn sync_registry_locked(&self) -> Result<bool> {
        let Some(runtime) = self.runtime.as_ref() else {
            return Ok(false);
        };
        let Some(load) = runtime.load_repos.as_ref() else {
            return Ok(false);
        };
        let Some(fresh) = load()? else {
            return Ok(false);
        };
        let fresh = dedup_by_slug(fresh);

        let mut canonical = runtime.repos.write().await;
        let live: HashMap<String, Arc<DaemonState>> = self.repos.read().await.clone();
        // A repo that failed to start last time is not live: keep retrying it.
        let in_step = fresh.iter().all(|e| live.contains_key(&e.slug))
            && live.keys().all(|s| fresh.iter().any(|e| &e.slug == s));
        if *canonical == fresh && in_step {
            return Ok(false);
        }

        for entry in &fresh {
            let prev = canonical.iter().find(|e| e.slug == entry.slug);
            match live.get(&entry.slug) {
                Some(ds) => {
                    let Some(prev) = prev else { continue };
                    if prev.features != entry.features && ds.features() != entry.features {
                        ds.set_features(entry.features.clone());
                        info!("registry sync: [{}] features reloaded", entry.slug);
                    }
                    if prev.apply != entry.apply {
                        let apply = entry.apply.unwrap_or(runtime.global_apply);
                        if ds.apply() != apply {
                            ds.set_apply(apply);
                            info!("registry sync: [{}] apply={apply}", entry.slug);
                        }
                    }
                }
                None => {
                    // New to this pod: same rules as boot.
                    let apply = entry.apply.unwrap_or(runtime.global_apply);
                    let features = effective_features(entry, apply);
                    match self.build_repo_state(runtime, entry, apply, features) {
                        Ok(state) => {
                            self.start_repo(runtime, &entry.slug, state).await;
                            info!("registry sync: [{}] added by another pod", entry.slug);
                        }
                        Err(e) => warn!("registry sync: cannot start [{}]: {e:#}", entry.slug),
                    }
                }
            }
        }
        for slug in live.keys() {
            if !fresh.iter().any(|e| &e.slug == slug) {
                if let Some(state) = self.repos.write().await.remove(slug) {
                    state.cancel();
                }
                info!("registry sync: [{slug}] removed by another pod");
            }
        }
        *canonical = fresh;
        Ok(true)
    }

    /// [`Self::sync_registry_locked`] that only logs: a failed refresh must
    /// not block a user's edit (it then falls back to this pod's view).
    async fn sync_registry_locked_logged(&self) {
        if let Err(e) = self.sync_registry_locked().await {
            warn!("registry sync failed: {e:#}");
        }
    }

    /// Build (but don't start) a repo's state from a registry entry, with the
    /// same preparation as boot (checkout for auto-fix, web password).
    fn build_repo_state(
        &self,
        runtime: &DynamicRuntime,
        entry: &crate::config::RepoEntry,
        apply: bool,
        features: crate::config::RepoFeatures,
    ) -> Result<Arc<DaemonState>> {
        ensure_checkout(entry, &features);
        let mut config = Config::load_for_repo(&entry.path, &entry.slug)?;
        std::fs::create_dir_all(&config.wshm_dir)?;
        config.web.resolve_password(&config.wshm_dir);
        let db = match &runtime.db_factory {
            Some(factory) => factory(&config)?,
            None => Arc::new(Database::open(&config)?) as Arc<dyn DatabaseBackend>,
        };
        let gh = Arc::new(GhClient::new(&config)?);
        Ok(Arc::new(DaemonState::with_features(
            db,
            gh,
            Arc::new(config),
            apply,
            features,
        )))
    }

    /// Insert a built repo state into the live map and spawn its scheduler
    /// (+ poller in polling mode). A state already registered under the slug
    /// is cancelled first so its loops never run twice.
    async fn start_repo(&self, runtime: &DynamicRuntime, slug: &str, state: Arc<DaemonState>) {
        let old = self
            .repos
            .write()
            .await
            .insert(slug.to_string(), Arc::clone(&state));
        if let Some(old) = old {
            if !Arc::ptr_eq(&old, &state) {
                old.cancel();
            }
        }

        let sched_state = Arc::clone(&state);
        tokio::spawn(async move {
            scheduler::run(sched_state).await;
        });

        if runtime.poll {
            let tx = runtime.event_tx.clone();
            let interval = Some(runtime.poll_interval);
            let slug_owned = slug.to_string();
            tokio::spawn(async move {
                poller::run_multi(state, tx, interval, slug_owned).await;
            });
        }
    }

    /// Add a repo at runtime: load config, build DaemonState, persist to
    /// global config, and spawn scheduler + poller (if enabled). Idempotent
    /// on the slug — returns error if it already exists.
    /// When `path` is None, defaults to `<global_config_parent>/repos/<name>`
    /// so dynamic adds land on the same volume as the daemon's config.
    pub async fn add_repo(&self, slug: &str, path: Option<PathBuf>) -> Result<Arc<DaemonState>> {
        let runtime = self
            .runtime
            .as_ref()
            .context("Dynamic add_repo not available (daemon not running in multi-repo mode)")?;

        // Validate the slug: exactly two non-empty owner/repo segments, and
        // no segment may be a path-traversal token (`.` / `..`) or contain a
        // separator, so a value like "owner/.." can never become a directory
        // component of ".." when we derive the on-disk path from it.
        let segments: Vec<&str> = slug.split('/').collect();
        if segments.len() != 2
            || segments
                .iter()
                .any(|s| s.is_empty() || *s == "." || *s == ".." || s.contains('\\'))
        {
            anyhow::bail!("invalid slug format, expected owner/repo");
        }

        let _registry = runtime.registry_lock.lock().await;
        self.sync_registry_locked_logged().await;

        let repos_base = runtime
            .global_config_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("repos");
        let path = match path {
            // A caller-supplied path is anchored under the repos/ base and must
            // not escape it (the web layer already rejects absolute paths and
            // ".." segments; re-check defensively here).
            Some(p) => {
                if p.is_absolute() || p.components().any(|c| c.as_os_str() == "..") {
                    anyhow::bail!("repo path must be relative and must not contain '..'");
                }
                repos_base.join(p)
            }
            None => {
                let name = segments[1];
                repos_base.join(name)
            }
        };

        {
            let repos = self.repos.read().await;
            if repos.contains_key(slug) {
                anyhow::bail!("repo {slug} already registered");
            }
        }

        let mut config = Config::load_for_repo(&path, slug)?;
        std::fs::create_dir_all(&config.wshm_dir)?;
        config.web.resolve_password(&config.wshm_dir);

        // Build the storage backend via the Pro-provided factory (shared
        // Postgres pool) when present; otherwise open a per-repo SQLite DB.
        // Using the factory is what makes a runtime-added repo land in
        // Postgres in prod instead of a stray, empty SQLite file.
        let db = match &runtime.db_factory {
            Some(factory) => factory(&config)?,
            None => Arc::new(Database::open(&config)?) as Arc<dyn DatabaseBackend>,
        };
        let gh = Arc::new(GhClient::new(&config)?);
        let state = Arc::new(DaemonState::new(
            db,
            gh,
            Arc::new(config),
            runtime.global_apply,
        ));

        // Persist before mutating runtime state so a crash can't lose the
        // user's intent silently. Append to the canonical registry, then flush
        // via the persistence hook (Pro → Postgres) or the TOML fallback.
        {
            let mut canonical = runtime.repos.write().await;
            canonical.push(crate::config::RepoEntry {
                slug: slug.to_string(),
                path: path.clone(),
                apply: None,
                enabled: true,
                secret: None,
                features: crate::config::RepoFeatures::default(),
            });
        }
        if let Err(e) = runtime.persist().await {
            runtime.repos.write().await.retain(|e| e.slug != slug);
            return Err(e).with_context(|| format!("failed to persist {slug} to repo registry"));
        }

        self.start_repo(runtime, slug, Arc::clone(&state)).await;

        info!(
            "Repo added at runtime: {} (path={}, apply={})",
            slug,
            path.display(),
            runtime.global_apply
        );

        Ok(state)
    }

    /// Remove a repo at runtime: stop its scheduler/poller, drop it from the
    /// live map and the canonical registry, and persist (Pro → Postgres, else
    /// TOML) so the removal survives a restart. Idempotent-ish: errors if the
    /// slug isn't registered.
    pub async fn remove_repo(&self, slug: &str) -> Result<()> {
        let runtime = self
            .runtime
            .as_ref()
            .context("Dynamic remove_repo not available (daemon not running in multi-repo mode)")?;

        let _registry = runtime.registry_lock.lock().await;
        self.sync_registry_locked_logged().await;

        // Drop from the live map and signal its background tasks to exit.
        let removed = {
            let mut repos = self.repos.write().await;
            repos.remove(slug)
        };
        let state = removed.with_context(|| format!("repo {slug} not registered"))?;
        state.cancel();

        // Remove from the canonical registry, then persist the new list.
        {
            let mut canonical = runtime.repos.write().await;
            canonical.retain(|e| e.slug != slug);
        }
        runtime
            .persist()
            .await
            .with_context(|| format!("failed to persist removal of {slug}"))?;

        info!("Repo removed at runtime: {slug}");
        Ok(())
    }
}

pub async fn run(mut config: Config, args: DaemonArgs) -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let apply = args.apply || config.daemon.apply;
    let bind = args
        .bind
        .clone()
        .unwrap_or_else(|| config.daemon.bind.clone());

    // Resolve web password (auto-generate if needed)
    config.web.resolve_password(&config.wshm_dir);

    let secret = args
        .secret
        .clone()
        .or_else(|| std::env::var("WSHM_WEBHOOK_SECRET").ok())
        .or_else(|| {
            if config.daemon.webhook_secret.is_some() {
                warn!("webhook_secret in config.toml is insecure — use WSHM_WEBHOOK_SECRET env var or .wshm/credentials instead");
            }
            config.daemon.webhook_secret.clone()
        });

    let db = Arc::new(Database::open(&config)?) as Arc<dyn DatabaseBackend>;
    let gh = Arc::new(GhClient::new(&config)?);
    let config = Arc::new(config);

    let (tx, rx) = mpsc::channel::<WebhookEvent>(WEBHOOK_CHANNEL_CAPACITY);

    let state = Arc::new(DaemonState::new(
        Arc::clone(&db),
        Arc::clone(&gh),
        Arc::clone(&config),
        apply,
    ));

    let mode = if args.poll { "polling" } else { "webhook" };
    info!(
        "Starting wshm daemon on {} (apply={}, mode={})",
        bind, apply, mode
    );

    // Spawn the event processor
    let processor_state = Arc::clone(&state);
    let processor_handle = tokio::spawn(async move {
        processor::run(processor_state, rx).await;
    });

    // Spawn the periodic scheduler
    let scheduler_state = Arc::clone(&state);
    let scheduler_handle = tokio::spawn(async move {
        scheduler::run(scheduler_state).await;
    });

    // Spawn the poller (if --poll)
    let poller_handle = if args.poll {
        let poller_state = Arc::clone(&state);
        let poller_tx = tx.clone();
        let interval = Some(args.poll_interval);
        Some(tokio::spawn(async move {
            poller::run(poller_state, poller_tx, interval).await;
        }))
    } else {
        None
    };

    // Spawn the HTTP server (unless --no-server)
    let server_handle = if !args.no_server {
        let server_state = Arc::clone(&state);
        let server_tx = tx.clone();
        Some(tokio::spawn(async move {
            let tls = config.web.resolve_tls();
            if let Err(e) =
                server::run(server_state, server_tx, &bind, secret.as_deref(), tls).await
            {
                tracing::error!("Server error: {e}");
            }
        }))
    } else {
        info!("HTTP server disabled (--no-server)");
        None
    };

    info!("Daemon running. Press Ctrl+C to stop.");

    // Wait for SIGINT or SIGTERM
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(e) => {
                warn!("Failed to register SIGTERM handler ({e}), falling back to Ctrl+C only");
                tokio::signal::ctrl_c().await.ok();
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
    }
    info!("Shutdown signal received, stopping...");

    if let Some(h) = server_handle {
        h.abort();
    }
    if let Some(h) = poller_handle {
        h.abort();
    }
    scheduler_handle.abort();
    drop(tx);

    let drain_timeout = std::time::Duration::from_secs(10);
    match tokio::time::timeout(drain_timeout, processor_handle).await {
        Ok(_) => info!("Processor drained cleanly."),
        Err(_) => warn!(
            "Processor did not drain within {}s.",
            drain_timeout.as_secs()
        ),
    }

    info!("Daemon stopped.");
    Ok(())
}

/// Pluggable extensions an external binary can pass into the multi-repo
/// daemon at startup. All fields default to `None`, in which case the daemon
/// behaves like the OSS build: no RBAC, no extra API routes, default SPA.
#[derive(Default)]
pub struct DaemonExtensions {
    /// Enable RBAC mode by passing a populated `UserStore`.
    pub users: Option<Arc<dyn crate::auth::UserStoreBackend>>,
    /// In-memory log buffer fed by the tracing layer; exposed via
    /// `GET /api/v1/logs`. Pass the same instance that's wired into the
    /// `tracing_subscriber` registry.
    pub logs: Option<Arc<log_buffer::LogBuffer>>,
    /// Encrypted secret store (AES-256-GCM, SQLite-backed). When `Some`,
    /// the `/api/v1/secrets/*` endpoints are served and the daemon will
    /// look up GitHub/Anthropic tokens here before falling back to env vars.
    pub secrets: Option<Arc<dyn crate::secrets::SecretStore>>,
    /// Extra API routes merged under the same auth layer as OSS routes.
    pub extra_api: Option<axum::Router<Arc<crate::daemon::web::WebState>>>,
    /// Replacement SPA router (e.g. a Pro web-dist with extra routes).
    pub spa_override: Option<axum::Router<Arc<crate::daemon::web::WebState>>>,
    /// Optional factory that produces the storage backend for one repo.
    /// When `Some`, the multi-repo daemon calls this instead of opening a
    /// per-repo SQLite `Database`. Pro uses it to share a single Postgres
    /// pool across all repos while scoping each backend instance by repo
    /// slug. When `None`, falls back to `Database::open(config)` (the
    /// historical OSS-only behaviour).
    pub db_factory: Option<DbFactory>,
    /// Optional hook to persist the full repo registry (Pro writes it to
    /// Postgres). When `Some`, runtime add/remove/feature edits call this
    /// instead of rewriting `global.toml`; when `None`, the TOML file is used.
    pub persist_repos: Option<RepoPersist>,
    /// Optional hook to re-read the persisted registry. When `Some`, every
    /// pod periodically reconciles with it (`[daemon] registry_sync_secs`),
    /// so multi-pod deployments see each other's settings changes live.
    pub load_repos: Option<RepoLoad>,
}

/// Produces the storage backend for one repo. Pro shares a single Postgres
/// pool across repos, scoping each instance by slug; OSS opens a per-repo
/// SQLite `Database`.
pub type DbFactory = Arc<dyn Fn(&Config) -> anyhow::Result<Arc<dyn DatabaseBackend>> + Send + Sync>;

/// Persists the whole tracked-repo registry (list + per-repo settings). Pro
/// writes the JSON blob to Postgres; OSS falls back to `global.toml`.
pub type RepoPersist = Arc<dyn Fn(&[crate::config::RepoEntry]) -> anyhow::Result<()> + Send + Sync>;

/// Reads the persisted tracked-repo registry back (Pro: the Postgres blob
/// [`RepoPersist`] writes). `Ok(None)` = nothing persisted yet.
pub type RepoLoad =
    Arc<dyn Fn() -> anyhow::Result<Option<Vec<crate::config::RepoEntry>>> + Send + Sync>;

/// Keep the first entry per slug: a duplicated slug would otherwise start
/// two schedulers/pollers for the same repo.
fn dedup_by_slug(entries: Vec<crate::config::RepoEntry>) -> Vec<crate::config::RepoEntry> {
    let mut out: Vec<crate::config::RepoEntry> = Vec::with_capacity(entries.len());
    for e in entries {
        if out.iter().any(|o| o.slug == e.slug) {
            warn!("repo registry: duplicate slug {} ignored", e.slug);
            continue;
        }
        out.push(e);
    }
    out
}

/// A local checkout is only needed by code-editing features (auto-fix PR
/// generation). Triage and PR analysis run entirely off the GitHub API, so
/// only clone when auto-fix is enabled and the checkout is missing (stateless
/// deploys mount repos/ as an emptyDir). Best-effort: failures are logged.
fn ensure_checkout(entry: &crate::config::RepoEntry, features: &crate::config::RepoFeatures) {
    if !features.auto_pr || entry.path.join(".git").exists() {
        return;
    }
    match std::env::var("GITHUB_TOKEN") {
        Ok(token) => {
            let url = format!("https://github.com/{}.git", entry.slug);
            if let Some(parent) = entry.path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            match crate::github::git::clone_repo(&url, &entry.path, &token) {
                Ok(_) => info!("Cloned {} into {}", entry.slug, entry.path.display()),
                Err(e) => warn!(
                    "Failed to clone {} into {} ({e}); daemon may degrade for this repo",
                    entry.slug,
                    entry.path.display()
                ),
            }
        }
        Err(_) => warn!(
            "Repo {} not present at {} and GITHUB_TOKEN unset — skipping clone, daemon may degrade",
            entry.slug,
            entry.path.display()
        ),
    }
}

/// Feature flags a registry entry runs with. A features section left at its
/// default is upgraded from the legacy `apply` flag so old setups keep
/// behaving the same — shared by boot and [`MultiDaemonState::sync_registry`].
fn effective_features(
    entry: &crate::config::RepoEntry,
    apply: bool,
) -> crate::config::RepoFeatures {
    let mut features = entry.features.clone();
    if features == crate::config::RepoFeatures::default() {
        features.merge_legacy_apply(apply);
    }
    features
}

/// Run daemon in multi-repo mode from a global config file.
pub async fn run_multi(global: GlobalConfig, args: DaemonArgs) -> Result<()> {
    run_multi_with_extensions(global, args, DaemonExtensions::default()).await
}

/// Run daemon in multi-repo mode with extension hooks for an external binary
/// to plug in RBAC, extra API routes, or a replacement SPA bundle. The OSS
/// binary calls [`run_multi`] which delegates here with empty extensions.
pub async fn run_multi_with_extensions(
    global: GlobalConfig,
    args: DaemonArgs,
    mut extensions: DaemonExtensions,
) -> Result<()> {
    // Fall back to the process-wide log buffer (set by `log_buffer::
    // install_global` in main.rs) so `/api/v1/logs` works even when the
    // caller didn't explicitly thread one through extensions.
    if extensions.logs.is_none() {
        extensions.logs = log_buffer::global();
    }

    // Install the retry policy process-wide so every outbound HTTP call
    // (poller, git providers, AI, self-update) honors it. The Settings UI
    // re-installs it on save, so this is just the boot-time default.
    crate::retry::set_global(global.retry.clone());

    // Open a default UserStore on ~/.wshm/users.db when the caller didn't
    // provide one, so OSS gets a working RBAC + login flow out of the box.
    // The Pro binary still passes its own (possibly Postgres-backed) store
    // and that takes precedence.
    if extensions.users.is_none() {
        let users_db = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".wshm")
            .join("users.db");
        match crate::auth::UserStore::open(&users_db) {
            Ok(store) => extensions.users = Some(Arc::new(store)),
            Err(e) => warn!(
                "Failed to open users db at {}: {e} — login disabled",
                users_db.display()
            ),
        }
    }

    // Install the secret store globally so non-async helpers like
    // Config::github_token can resolve from the encrypted store before
    // falling back to env vars.
    if let Some(store) = extensions.secrets.as_ref() {
        crate::secrets::install_global(Arc::clone(store));
    }
    // Install rustls crypto provider early (needed even before TLS handshake)
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    // Boot-time security checks: warn loudly when the daemon is running
    // in a configuration that delegates auth to an upstream proxy
    // (oauth2-proxy / Cloudflare Access) without a network boundary
    // protecting it. The trusted-headers shortcut is unauthenticated
    // by itself; only a NetworkPolicy / firewall makes it safe.
    let trust_proxy = std::env::var("WSHM_TRUST_PROXY_AUTH")
        .ok()
        .filter(|v| v == "1" || v == "true")
        .is_some();
    if trust_proxy {
        let bind_addr = args
            .bind
            .clone()
            .unwrap_or_else(|| global.daemon.bind.clone());
        if !bind_addr.starts_with("127.") && !bind_addr.starts_with("[::1]") {
            warn!(
                "WSHM_TRUST_PROXY_AUTH=1 with a non-loopback bind ({bind_addr}). \
                 Make sure ONLY your reverse proxy can reach this port — anyone \
                 else can forge X-Forwarded-Email and bypass auth. On K8s, see \
                 deploy/k8s/networkpolicy.yaml."
            );
        }
        let has_token = std::env::var("WSHM_PROXY_AUTH_TOKEN")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        let isolation_ack = std::env::var("WSHM_TRUST_PROXY_AUTH_NO_TOKEN")
            .ok()
            .filter(|v| v == "1" || v == "true")
            .is_some();
        if !has_token && !isolation_ack {
            warn!(
                "WSHM_TRUST_PROXY_AUTH=1 but WSHM_PROXY_AUTH_TOKEN is unset. \
                 Forwarded-identity headers will be REJECTED (fail closed). \
                 Set WSHM_PROXY_AUTH_TOKEN to a shared secret your proxy adds as \
                 X-Wshm-Proxy-Token, or set WSHM_TRUST_PROXY_AUTH_NO_TOKEN=1 to \
                 explicitly rely on network isolation alone."
            );
        }
    }

    let global_apply = args.apply || global.daemon.apply;
    let bind = args
        .bind
        .clone()
        .unwrap_or_else(|| global.daemon.bind.clone());

    let secret = args
        .secret
        .clone()
        .or_else(|| std::env::var("WSHM_WEBHOOK_SECRET").ok())
        .or_else(|| global.daemon.webhook_secret.clone());

    let poll = args.poll || global.daemon.poll;
    let poll_interval = if args.poll_interval != 30 {
        args.poll_interval
    } else {
        global.daemon.poll_interval
    };

    // Build a DaemonState per repo
    let mut repos = HashMap::new();
    let mut web_password_resolved = false;
    for entry in &global.repos {
        // Clone only when auto-fix needs a checkout and it is missing.
        ensure_checkout(entry, &entry.features);

        let mut config = Config::load_for_repo(&entry.path, &entry.slug)?;

        // Ensure .wshm dir exists
        std::fs::create_dir_all(&config.wshm_dir)?;

        // Resolve web password on the first repo (shared across all)
        if !web_password_resolved {
            config.web.resolve_password(&config.wshm_dir);
            web_password_resolved = true;
        }

        let db = match &extensions.db_factory {
            Some(factory) => factory(&config)?,
            None => Arc::new(Database::open(&config)?) as Arc<dyn DatabaseBackend>,
        };
        let gh = Arc::new(GhClient::new(&config)?);
        let apply = entry.apply.unwrap_or(global_apply);

        // If features section was present in global.toml, use it; else
        // derive from the legacy `apply` flag so existing setups stay
        // backward-compatible.
        let features = effective_features(entry, apply);
        let state = Arc::new(DaemonState::with_features(
            db,
            gh,
            Arc::new(config),
            apply,
            features,
        ));

        info!(
            "Loaded repo: {} (path={}, apply={})",
            entry.slug,
            entry.path.display(),
            apply
        );
        repos.insert(entry.slug.clone(), state);
    }

    let (tx, rx) = mpsc::channel::<(String, WebhookEvent)>(WEBHOOK_CHANNEL_CAPACITY);

    let global_config_path = args
        .config
        .clone()
        .unwrap_or_else(GlobalConfig::default_path);

    let runtime = DynamicRuntime {
        event_tx: tx.clone(),
        poll,
        poll_interval,
        global_apply,
        global_config_path,
        repos: Arc::new(RwLock::new(global.repos.clone())),
        persist_repos: extensions.persist_repos.clone(),
        load_repos: extensions.load_repos.clone(),
        registry_lock: Arc::new(tokio::sync::Mutex::new(())),
        db_factory: extensions.db_factory.clone(),
    };

    let multi = Arc::new(MultiDaemonState::with_runtime(repos, runtime));

    // Multi-pod: periodically reconcile with the shared registry so a
    // features/apply/repo change made through another pod lands here
    // without a restart. No-op without a `load_repos` hook (OSS).
    if extensions.load_repos.is_some() && global.daemon.registry_sync_secs > 0 {
        let every = std::time::Duration::from_secs(global.daemon.registry_sync_secs);
        let sync_multi = Arc::clone(&multi);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                if let Err(e) = sync_multi.sync_registry().await {
                    warn!("registry sync failed: {e:#}");
                }
            }
        });
    }

    let repo_count = multi.repos.read().await.len();
    info!(
        "Starting multi-repo daemon on {} ({} repos, apply={}, mode={})",
        bind,
        repo_count,
        global_apply,
        if poll { "polling" } else { "webhook" }
    );

    // Spawn the multi-repo event processor
    let processor_multi = Arc::clone(&multi);
    let processor_handle = tokio::spawn(async move {
        processor::run_multi(processor_multi, rx).await;
    });

    // Spawn a scheduler per repo
    let mut scheduler_handles = Vec::new();
    {
        let repos = multi.repos.read().await;
        for state in repos.values() {
            let s = Arc::clone(state);
            scheduler_handles.push(tokio::spawn(async move {
                scheduler::run(s).await;
            }));
        }
    }

    // Spawn a poller per repo (if --poll)
    let mut poller_handles = Vec::new();
    if poll {
        let repos = multi.repos.read().await;
        for (slug, state) in repos.iter() {
            let s = Arc::clone(state);
            let t = tx.clone();
            let slug = slug.clone();
            let interval = Some(poll_interval);
            poller_handles.push(tokio::spawn(async move {
                poller::run_multi(s, t, interval, slug).await;
            }));
        }
    }

    // Spawn the HTTP server (unless --no-server)
    let server_handle = if !args.no_server {
        let server_multi = Arc::clone(&multi);
        let server_tx = tx.clone();
        Some(tokio::spawn(async move {
            // TLS: resolve from env vars (multi-repo doesn't have per-repo web config)
            let tls = {
                let cert = std::env::var("WSHM_TLS_CERT")
                    .ok()
                    .filter(|s| !s.is_empty());
                let key = std::env::var("WSHM_TLS_KEY").ok().filter(|s| !s.is_empty());
                match (cert, key) {
                    (Some(c), Some(k)) => Some((c, k)),
                    _ => None,
                }
            };
            if let Err(e) = server::run_multi(
                server_multi,
                server_tx,
                &bind,
                secret.as_deref(),
                tls,
                extensions,
            )
            .await
            {
                tracing::error!("Server error: {e}");
            }
        }))
    } else {
        info!("HTTP server disabled (--no-server)");
        None
    };

    // Spawn a single global auto-update task (not per-repo)
    let update_handle = if global.update.enabled {
        let interval_hours = global.update.interval_hours;
        info!("Auto-update enabled (every {interval_hours}h, checking now...)");
        Some(tokio::spawn(async move {
            // Check immediately on startup
            crate::pro_hooks::run_auto_update().await;
            let interval = std::time::Duration::from_secs(interval_hours as u64 * 3600);
            loop {
                tokio::time::sleep(interval).await;
                crate::pro_hooks::run_auto_update().await;
            }
        }))
    } else {
        None
    };

    let repo_count = multi.repos.read().await.len();
    info!(
        "Multi-repo daemon running ({} repos). Press Ctrl+C to stop.",
        repo_count
    );

    // Wait for SIGINT (Ctrl+C) or SIGTERM (systemd/docker)
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sigterm) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = sigterm.recv() => {}
                }
            }
            Err(e) => {
                warn!("Failed to register SIGTERM handler ({e}), falling back to Ctrl+C only");
                tokio::signal::ctrl_c().await.ok();
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
    }
    info!("Shutdown signal received, stopping...");

    // Stop accepting new events: abort server, pollers, schedulers, update
    if let Some(h) = server_handle {
        h.abort();
    }
    if let Some(h) = update_handle {
        h.abort();
    }
    for h in poller_handles {
        h.abort();
    }
    for h in scheduler_handles {
        h.abort();
    }

    // Drop the sender so the processor's recv() returns None and it can drain in-flight tasks
    drop(tx);

    // Give the processor up to 10s to finish in-flight events
    let drain_timeout = std::time::Duration::from_secs(10);
    match tokio::time::timeout(drain_timeout, processor_handle).await {
        Ok(_) => info!("Processor drained cleanly."),
        Err(_) => {
            warn!(
                "Processor did not drain within {}s, aborting.",
                drain_timeout.as_secs()
            );
        }
    }

    info!("Multi-repo daemon stopped.");
    Ok(())
}

#[cfg(test)]
mod registry_sync_tests {
    use super::*;
    use crate::config::RepoEntry;
    use std::path::Path;
    use std::sync::Mutex;

    type Store = Arc<Mutex<Option<Vec<RepoEntry>>>>;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "wshm-regsync-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(d.join(".wshm")).unwrap();
        d
    }

    fn entry(slug: &str, path: &Path) -> RepoEntry {
        RepoEntry {
            slug: slug.to_string(),
            path: path.to_path_buf(),
            apply: None,
            enabled: true,
            secret: None,
            features: crate::config::RepoFeatures::default(),
        }
    }

    /// Blocks the NEXT persist() inside the hook (after its snapshot was
    /// taken) until released, so a test can run a sync in that window.
    struct Gate {
        armed: std::sync::atomic::AtomicBool,
        entered: std::sync::mpsc::SyncSender<()>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    fn gate() -> (
        Arc<Gate>,
        std::sync::mpsc::Receiver<()>,
        std::sync::mpsc::SyncSender<()>,
    ) {
        let (etx, erx) = std::sync::mpsc::sync_channel(1);
        let (rtx, rrx) = std::sync::mpsc::sync_channel(1);
        let g = Gate {
            armed: std::sync::atomic::AtomicBool::new(true),
            entered: etx,
            release: Mutex::new(rrx),
        };
        (Arc::new(g), erx, rtx)
    }

    fn pod(store: &Store, root: &Path) -> MultiDaemonState {
        pod_gated(store, root, None)
    }

    /// One "pod": its own live map + canonical copy, sharing `store` (the
    /// stand-in for the Postgres registry blob) through the persist/load hooks.
    fn pod_gated(store: &Store, root: &Path, gate: Option<Arc<Gate>>) -> MultiDaemonState {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let initial = store.lock().unwrap().clone().unwrap_or_default();
        let (tx, _rx) = mpsc::channel(8);
        let (s1, s2) = (Arc::clone(store), Arc::clone(store));
        let runtime = DynamicRuntime {
            event_tx: tx,
            poll: false,
            poll_interval: 30,
            global_apply: false,
            global_config_path: root.join("global.toml"),
            repos: Arc::new(RwLock::new(initial.clone())),
            persist_repos: Some(Arc::new(move |r: &[RepoEntry]| {
                if let Some(g) = &gate {
                    if g.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                        g.entered.send(()).ok();
                        g.release.lock().unwrap().recv().ok();
                    }
                }
                *s1.lock().unwrap() = Some(r.to_vec());
                Ok(())
            })),
            load_repos: Some(Arc::new(move || Ok(s2.lock().unwrap().clone()))),
            registry_lock: Arc::new(tokio::sync::Mutex::new(())),
            db_factory: None,
        };
        let mut live = HashMap::new();
        for e in &initial {
            let mut config = Config::load_for_repo(&e.path, &e.slug).unwrap();
            config.wshm_dir = e.path.join(".wshm");
            let apply = e.apply.unwrap_or(false);
            let state = DaemonState::with_features(
                Arc::new(Database::open_memory().unwrap()),
                Arc::new(GhClient::new(&config).unwrap()),
                Arc::new(config),
                apply,
                effective_features(e, apply),
            );
            live.insert(e.slug.clone(), Arc::new(state));
        }
        MultiDaemonState::with_runtime(live, runtime)
    }

    async fn live(m: &MultiDaemonState, slug: &str) -> Option<Arc<DaemonState>> {
        m.repos.read().await.get(slug).cloned()
    }

    #[tokio::test]
    async fn sync_noop_when_registry_unchanged() {
        let root = tmpdir("noop");
        let store: Store = Arc::new(Mutex::new(Some(vec![entry("o/a", &root)])));
        let p = pod(&store, &root);
        assert!(!p.sync_registry().await.unwrap());
        // No hook (OSS) → never syncs.
        let none = MultiDaemonState::new(HashMap::new());
        assert!(!none.sync_registry().await.unwrap());
    }

    #[tokio::test]
    async fn features_and_apply_change_on_other_pod_propagate() {
        let root = tmpdir("feat");
        let store: Store = Arc::new(Mutex::new(Some(vec![entry("o/a", &root)])));
        let p1 = pod(&store, &root);
        let ds = live(&p1, "o/a").await.unwrap();
        assert!(!ds.features().auto_merge);

        // "Pod 2" edits the shared registry.
        {
            let mut g = store.lock().unwrap();
            let e = &mut g.as_mut().unwrap()[0];
            e.features.triage_issues = true;
            e.features.auto_merge = true;
            e.apply = Some(true);
        }
        assert!(p1.sync_registry().await.unwrap());
        assert!(ds.features().auto_merge && ds.features().triage_issues);
        assert!(ds.apply());
        // Converged: next cycle is a no-op.
        assert!(!p1.sync_registry().await.unwrap());
    }

    #[tokio::test]
    async fn repo_added_and_removed_on_other_pod_propagate() {
        let root = tmpdir("addrm");
        let other = tmpdir("addrm-b");
        let store: Store = Arc::new(Mutex::new(Some(vec![entry("o/a", &root)])));
        let p1 = pod(&store, &root);
        let a = live(&p1, "o/a").await.unwrap();

        *store.lock().unwrap() = Some(vec![entry("o/b", &other)]);
        assert!(p1.sync_registry().await.unwrap());
        assert!(live(&p1, "o/a").await.is_none(), "removed elsewhere");
        assert!(a.is_cancelled(), "removed repo's loops are stopped");
        let b = live(&p1, "o/b").await.expect("added elsewhere is started");
        b.cancel();
    }

    /// A mutation on a stale pod must not revert another pod's edit when it
    /// persists the whole registry.
    #[tokio::test]
    async fn mutation_on_stale_pod_keeps_other_pods_edit() {
        let root = tmpdir("lost");
        let store: Store = Arc::new(Mutex::new(Some(vec![entry("o/a", &root)])));
        let p1 = pod(&store, &root);
        store.lock().unwrap().as_mut().unwrap()[0]
            .features
            .auto_merge = true;

        // p1 (stale) removes nothing but adds a repo → persists the registry.
        let added = p1.add_repo("o/new", None).await.unwrap();
        added.cancel();
        let persisted = store.lock().unwrap().clone().unwrap();
        let a = persisted.iter().find(|e| e.slug == "o/a").unwrap();
        assert!(a.features.auto_merge, "other pod's edit survived");
        assert!(persisted.iter().any(|e| e.slug == "o/new"));
    }
    fn canonical_slugs(m: &MultiDaemonState) -> Vec<String> {
        let rt = m.runtime.as_ref().unwrap();
        rt.repos
            .try_read()
            .unwrap()
            .iter()
            .map(|e| e.slug.clone())
            .collect()
    }

    /// Run `op` (a registry mutation) and fire a periodic-style sync while
    /// `op` is inside persist(): the exact window the reviewer reproduced.
    async fn race_sync_during_persist<F>(
        p: &Arc<MultiDaemonState>,
        entered: std::sync::mpsc::Receiver<()>,
        release: std::sync::mpsc::SyncSender<()>,
        op: F,
    ) where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let op = tokio::spawn(op);
        tokio::task::spawn_blocking(move || entered.recv())
            .await
            .unwrap()
            .unwrap();
        let p2 = Arc::clone(p);
        let sync = tokio::spawn(async move { p2.sync_registry().await });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        release.send(()).unwrap();
        op.await.unwrap();
        sync.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sync_during_features_patch_does_not_revert_it() {
        let root = tmpdir("race-patch");
        let store: Store = Arc::new(Mutex::new(Some(vec![entry("o/a", &root)])));
        let (g, entered, release) = gate();
        let p = Arc::new(pod_gated(&store, &root, Some(g)));
        let p2 = Arc::clone(&p);
        // Same lock protocol as api_repo_features_patch.
        race_sync_during_persist(&p, entered, release, async move {
            let _g = p2.lock_registry().await;
            p2.sync_registry_locked().await.unwrap();
            let ds = live(&p2, "o/a").await.unwrap();
            let mut f = ds.features();
            f.auto_merge = true;
            ds.set_features(f.clone());
            let rt = p2.runtime.as_ref().unwrap();
            rt.repos.write().await[0].features = f;
            rt.persist().await.unwrap();
        })
        .await;
        let ds = live(&p, "o/a").await.unwrap();
        assert!(ds.features().auto_merge, "in-memory edit kept");
        assert!(
            store.lock().unwrap().as_ref().unwrap()[0]
                .features
                .auto_merge
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sync_during_add_repo_does_not_lose_it() {
        let root = tmpdir("race-add");
        let store: Store = Arc::new(Mutex::new(Some(vec![entry("o/a", &root)])));
        let (g, entered, release) = gate();
        let p = Arc::new(pod_gated(&store, &root, Some(g)));
        let p2 = Arc::clone(&p);
        race_sync_during_persist(&p, entered, release, async move {
            p2.add_repo("o/new", None).await.unwrap();
        })
        .await;
        assert!(canonical_slugs(&p).contains(&"o/new".to_string()));
        assert!(store
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .iter()
            .any(|e| e.slug == "o/new"));
        live(&p, "o/new").await.expect("running").cancel();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sync_during_remove_repo_does_not_resurrect_it() {
        let root = tmpdir("race-rm");
        let store: Store = Arc::new(Mutex::new(Some(vec![entry("o/a", &root)])));
        let (g, entered, release) = gate();
        let p = Arc::new(pod_gated(&store, &root, Some(g)));
        let p2 = Arc::clone(&p);
        race_sync_during_persist(&p, entered, release, async move {
            p2.remove_repo("o/a").await.unwrap();
        })
        .await;
        assert!(live(&p, "o/a").await.is_none(), "not resurrected");
        assert!(canonical_slugs(&p).is_empty());
        assert!(store.lock().unwrap().as_ref().unwrap().is_empty());
    }

    /// apply=true repo whose user turned features off: an unrelated registry
    /// change must not re-run the legacy upgrade and re-enable them live.
    #[tokio::test]
    async fn hot_reload_never_reapplies_legacy_upgrade() {
        let root = tmpdir("legacy");
        let mut a = entry("o/a", &root);
        a.apply = Some(true);
        let store: Store = Arc::new(Mutex::new(Some(vec![a])));
        let p = pod(&store, &root);
        let ds = live(&p, "o/a").await.unwrap();
        assert!(ds.features().triage_issues, "boot upgrade applied");
        // User disables everything (stored features == default).
        ds.set_features(crate::config::RepoFeatures::default());
        // Unrelated change to the same entry elsewhere.
        store.lock().unwrap().as_mut().unwrap()[0].secret = Some("x".into());
        assert!(p.sync_registry().await.unwrap());
        assert_eq!(ds.features(), crate::config::RepoFeatures::default());
    }

    #[tokio::test]
    async fn duplicate_slugs_start_once_and_restart_cancels_old() {
        let root = tmpdir("dup");
        let store: Store = Arc::new(Mutex::new(Some(vec![])));
        let p = pod(&store, &root);
        *store.lock().unwrap() = Some(vec![entry("o/a", &root), entry("o/a", &root)]);
        assert!(p.sync_registry().await.unwrap());
        assert_eq!(p.repos.read().await.len(), 1);
        assert_eq!(canonical_slugs(&p), vec!["o/a"]);
        assert!(
            !p.sync_registry().await.unwrap(),
            "deduped registry converges"
        );

        let first = live(&p, "o/a").await.unwrap();
        let rt = p.runtime.as_ref().unwrap();
        let again = p
            .build_repo_state(rt, &entry("o/a", &root), false, Default::default())
            .unwrap();
        p.start_repo(rt, "o/a", Arc::clone(&again)).await;
        assert!(first.is_cancelled(), "replaced state's loops stopped");
        again.cancel();
    }

    #[tokio::test]
    async fn repo_that_failed_to_start_is_retried() {
        let root = tmpdir("retry");
        let store: Store = Arc::new(Mutex::new(Some(vec![])));
        let p = pod(&store, &root);
        // Invalid slug → Config::load_for_repo fails.
        *store.lock().unwrap() = Some(vec![entry("broken", &root)]);
        assert!(p.sync_registry().await.unwrap());
        assert!(live(&p, "broken").await.is_none());
        assert!(
            p.sync_registry().await.unwrap(),
            "not live yet: next cycle retries instead of no-op"
        );
    }
}
