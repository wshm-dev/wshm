pub mod commands;
pub mod memory;
pub mod poller;
pub mod processor;
pub mod scheduler;
pub mod server;
pub mod systemd;
pub mod web;

/// Maximum number of webhook events buffered in memory before backpressure.
const WEBHOOK_CHANNEL_CAPACITY: usize = 256;

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::cli::DaemonArgs;
use crate::config::{Config, GlobalConfig};
use crate::db::Database;
use crate::github::Client as GhClient;

use self::processor::WebhookEvent;

pub struct DaemonState {
    pub db: Arc<Database>,
    pub gh: Arc<GhClient>,
    pub config: Arc<Config>,
    pub apply: bool,
}

/// Multi-repo state: maps "owner/repo" slug to its DaemonState.
pub struct MultiDaemonState {
    pub repos: HashMap<String, Arc<DaemonState>>,
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

    let db = Arc::new(Database::open(&config)?);
    let gh = Arc::new(GhClient::new(&config)?);
    let config = Arc::new(config);

    let (tx, rx) = mpsc::channel::<WebhookEvent>(WEBHOOK_CHANNEL_CAPACITY);

    let state = Arc::new(DaemonState {
        db: Arc::clone(&db),
        gh: Arc::clone(&gh),
        config: Arc::clone(&config),
        apply,
    });

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

/// Run daemon in multi-repo mode from a global config file.
pub async fn run_multi(global: GlobalConfig, args: DaemonArgs) -> Result<()> {
    // Install rustls crypto provider early (needed even before TLS handshake)
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

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
        let mut config = Config::load_for_repo(&entry.path, &entry.slug)?;

        // Ensure .wshm dir exists
        std::fs::create_dir_all(&config.wshm_dir)?;

        // Resolve web password on the first repo (shared across all)
        if !web_password_resolved {
            config.web.resolve_password(&config.wshm_dir);
            web_password_resolved = true;
        }

        let db = Arc::new(Database::open(&config)?);
        let gh = Arc::new(GhClient::new(&config)?);
        let apply = entry.apply.unwrap_or(global_apply);

        let state = Arc::new(DaemonState {
            db,
            gh,
            config: Arc::new(config),
            apply,
        });

        info!(
            "Loaded repo: {} (path={}, apply={})",
            entry.slug,
            entry.path.display(),
            apply
        );
        repos.insert(entry.slug.clone(), state);
    }

    let multi = Arc::new(MultiDaemonState { repos });

    let (tx, rx) = mpsc::channel::<(String, WebhookEvent)>(WEBHOOK_CHANNEL_CAPACITY);

    info!(
        "Starting multi-repo daemon on {} ({} repos, apply={}, mode={})",
        bind,
        multi.repos.len(),
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
    for state in multi.repos.values() {
        let s = Arc::clone(state);
        scheduler_handles.push(tokio::spawn(async move {
            scheduler::run(s).await;
        }));
    }

    // Spawn a poller per repo (if --poll)
    let mut poller_handles = Vec::new();
    if poll {
        for (slug, state) in &multi.repos {
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
            if let Err(e) =
                server::run_multi(server_multi, server_tx, &bind, secret.as_deref(), tls).await
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

    info!(
        "Multi-repo daemon running ({} repos). Press Ctrl+C to stop.",
        multi.repos.len()
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
