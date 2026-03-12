pub mod commands;
pub mod memory;
pub mod poller;
pub mod processor;
pub mod scheduler;
pub mod server;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;

use crate::cli::DaemonArgs;
use crate::config::Config;
use crate::db::Database;
use crate::github::Client as GhClient;

use self::processor::WebhookEvent;

pub struct DaemonState {
    pub db: Arc<Database>,
    pub gh: Arc<GhClient>,
    pub config: Arc<Config>,
    pub apply: bool,
}

pub async fn run(config: Config, args: DaemonArgs) -> Result<()> {
    let apply = args.apply || config.daemon.apply;
    let bind = args
        .bind
        .clone()
        .unwrap_or_else(|| config.daemon.bind.clone());

    let secret = args
        .secret
        .clone()
        .or_else(|| std::env::var("WSHM_WEBHOOK_SECRET").ok())
        .or_else(|| config.daemon.webhook_secret.clone());

    let db = Arc::new(Database::open(&config)?);
    let gh = Arc::new(GhClient::new(&config)?);
    let config = Arc::new(config);

    let (tx, rx) = mpsc::channel::<WebhookEvent>(256);

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
        Some(tokio::spawn(async move {
            if let Err(e) = server::run(server_state, tx, &bind, secret.as_deref()).await {
                tracing::error!("Server error: {e}");
            }
        }))
    } else {
        info!("HTTP server disabled (--no-server)");
        None
    };

    info!("Daemon running. Press Ctrl+C to stop.");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received, stopping...");

    // Abort spawned tasks
    if let Some(h) = server_handle {
        h.abort();
    }
    if let Some(h) = poller_handle {
        h.abort();
    }
    processor_handle.abort();
    scheduler_handle.abort();

    info!("Daemon stopped.");
    Ok(())
}
