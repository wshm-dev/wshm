use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info};

use crate::cli::TriageArgs;
use crate::github;
use crate::pipelines;
use crate::update;

use super::DaemonState;

pub async fn run(state: Arc<DaemonState>) {
    let interval = Duration::from_secs(state.config.sync.interval_minutes as u64 * 60);
    let update_interval =
        Duration::from_secs(state.config.update.interval_hours as u64 * 3600);
    let mut last_update_check = Instant::now();

    info!(
        "Scheduler started (sync every {}m)",
        state.config.sync.interval_minutes
    );

    if state.config.update.enabled {
        info!(
            "Auto-update enabled (every {}h, checking now...)",
            state.config.update.interval_hours
        );
        update::auto_check_and_update().await;
    }

    loop {
        tokio::time::sleep(interval).await;

        info!("Periodic sync triggered");
        match github::sync::full_sync(&state.gh, &state.db).await {
            Ok(_) => info!("Periodic sync complete"),
            Err(e) => {
                error!("Periodic sync failed: {e:#}");
                continue;
            }
        }

        // Triage untriaged issues after sync
        if state.config.triage.enabled {
            let args = TriageArgs {
                issue: None,
                apply: state.apply,
            };

            match pipelines::triage::run(&state.config, &state.db, &state.gh, &args, false).await {
                Ok(()) => info!("Scheduled triage complete"),
                Err(e) => error!("Scheduled triage failed: {e:#}"),
            }
        }

        // Auto-update check
        if state.config.update.enabled && last_update_check.elapsed() >= update_interval {
            last_update_check = Instant::now();
            update::auto_check_and_update().await;
        }
    }
}
