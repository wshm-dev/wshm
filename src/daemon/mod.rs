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
        Self {
            db,
            gh: std::sync::RwLock::new(gh),
            config,
            apply: std::sync::atomic::AtomicBool::new(apply),
            features: std::sync::RwLock::new(features),
        }
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

        let db = Arc::new(Database::open(&config)?) as Arc<dyn DatabaseBackend>;
        let gh = Arc::new(GhClient::new(&config)?);
        let state = Arc::new(DaemonState::new(
            db,
            gh,
            Arc::new(config),
            runtime.global_apply,
        ));

        // Persist before mutating runtime state so a crash can't lose the
        // user's intent silently.
        crate::config::append_repo_to_global(&runtime.global_config_path, slug, &path, None)
            .with_context(|| format!("failed to persist {slug} to global config"))?;

        {
            let mut repos = self.repos.write().await;
            repos.insert(slug.to_string(), Arc::clone(&state));
        }

        // Spawn scheduler for this repo
        let sched_state = Arc::clone(&state);
        tokio::spawn(async move {
            scheduler::run(sched_state).await;
        });

        // Spawn poller if polling mode is enabled
        if runtime.poll {
            let poll_state = Arc::clone(&state);
            let tx = runtime.event_tx.clone();
            let interval = Some(runtime.poll_interval);
            let slug_owned = slug.to_string();
            tokio::spawn(async move {
                poller::run_multi(poll_state, tx, interval, slug_owned).await;
            });
        }

        info!(
            "Repo added at runtime: {} (path={}, apply={})",
            slug,
            path.display(),
            runtime.global_apply
        );

        Ok(state)
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
    #[allow(clippy::type_complexity)]
    pub db_factory: Option<
        Arc<
            dyn Fn(&crate::Config) -> anyhow::Result<Arc<dyn crate::db::backend::DatabaseBackend>>
                + Send
                + Sync,
        >,
    >,
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
        // A local checkout is only needed by code-editing features
        // (auto-fix PR generation). Triage and PR analysis run entirely
        // off the GitHub API, so skip the boot clone unless auto-fix is
        // enabled for this repo — it's slow, depends on a valid
        // GITHUB_TOKEN, and its failure otherwise logs a misleading
        // "daemon may degrade" for repos that never needed the checkout.
        // Stateless deploys mount repos/ as an emptyDir, so when it IS
        // needed the checkout may be missing on first boot; clone it here.
        // Skipped when the entry path already contains a checkout.
        if entry.features.auto_pr && !entry.path.join(".git").exists() {
            if let Ok(token) = std::env::var("GITHUB_TOKEN") {
                let url = format!("https://github.com/{}.git", entry.slug);
                if let Some(parent) = entry.path.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                match crate::github::git::clone_repo(&url, &entry.path, &token) {
                    Ok(_) => info!("Cloned {} into {}", entry.slug, entry.path.display()),
                    Err(e) => {
                        tracing::warn!(
                            "Failed to clone {} into {} ({e}); daemon may degrade for this repo",
                            entry.slug,
                            entry.path.display()
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "Repo {} not present at {} and GITHUB_TOKEN unset — skipping clone, daemon may degrade",
                    entry.slug,
                    entry.path.display()
                );
            }
        }

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
        let mut features = entry.features.clone();
        if features == crate::config::RepoFeatures::default() {
            features.merge_legacy_apply(apply);
        }
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
    };

    let multi = Arc::new(MultiDaemonState::with_runtime(repos, runtime));

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
