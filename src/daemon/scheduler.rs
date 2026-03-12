use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

use crate::cli::TriageArgs;
use crate::github;
use crate::pipelines;

use super::DaemonState;

pub async fn run(state: Arc<DaemonState>) {
    let interval = Duration::from_secs(state.config.sync.interval_minutes as u64 * 60);

    info!(
        "Scheduler started (sync every {}m)",
        state.config.sync.interval_minutes
    );

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
    }
}
