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
    let update_interval = Duration::from_secs(state.config.update.interval_hours as u64 * 3600);
    let retriage_interval_hours = state.config.triage.retriage_interval_hours;
    let retriage_interval = Duration::from_secs(retriage_interval_hours as u64 * 3600);

    let mut last_update_check = Instant::now();
    let mut last_retriage = Instant::now();

    info!(
        "Scheduler started (sync every {}m)",
        state.config.sync.interval_minutes
    );

    if retriage_interval_hours > 0 {
        info!("Retriage enabled (every {retriage_interval_hours}h)");
    }

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
                retriage: false,
            };

            match pipelines::triage::run(&state.config, &state.db, &state.gh, &args, false, None)
                .await
            {
                Ok(()) => info!("Scheduled triage complete"),
                Err(e) => error!("Scheduled triage failed: {e:#}"),
            }
        }

        // Periodic retriage: re-evaluate stale triage results
        if state.config.triage.enabled
            && retriage_interval_hours > 0
            && last_retriage.elapsed() >= retriage_interval
        {
            last_retriage = Instant::now();
            info!("Periodic retriage triggered (interval: {retriage_interval_hours}h)");

            let args = TriageArgs {
                issue: None,
                apply: state.apply,
                retriage: true,
            };

            match pipelines::triage::run(&state.config, &state.db, &state.gh, &args, false, None)
                .await
            {
                Ok(()) => info!("Scheduled retriage complete"),
                Err(e) => error!("Scheduled retriage failed: {e:#}"),
            }
        }

        // Auto-update check
        if state.config.update.enabled && last_update_check.elapsed() >= update_interval {
            last_update_check = Instant::now();
            update::auto_check_and_update().await;
        }
    }
}
