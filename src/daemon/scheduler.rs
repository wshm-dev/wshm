use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

use crate::cli::TriageArgs;
use crate::github;
use crate::pipelines;
use crate::update;

use super::DaemonState;

/// Number of consecutive failures before sending an alert.
const ALERT_THRESHOLD: u32 = 3;

pub async fn run(state: Arc<DaemonState>) {
    let interval = Duration::from_secs(state.config.sync.interval_minutes as u64 * 60);
    let update_interval = Duration::from_secs(state.config.update.interval_hours as u64 * 3600);
    let retriage_interval_hours = state.config.triage.retriage_interval_hours;
    let retriage_interval = Duration::from_secs(retriage_interval_hours as u64 * 3600);

    let mut last_update_check = Instant::now();
    let mut last_retriage = Instant::now();
    let mut last_daily_notify = Instant::now();
    let daily_notify_interval = Duration::from_secs(24 * 3600);

    // Failure counters
    let mut sync_failures: u32 = 0;
    let mut triage_failures: u32 = 0;
    let mut sync_alert_sent = false;
    let mut triage_alert_sent = false;

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

        info!("Periodic sync triggered (incremental)");
        match github::sync::incremental_sync_full(&state.gh, &state.db).await {
            Ok(_) => {
                info!("Periodic sync complete");

                // Reset sync failure counter
                if sync_alert_sent {
                    // OSS: alert via log only (Pro has notify pipeline)
                    warn!("Sync recovered after {} failures", sync_failures);
                    sync_alert_sent = false;
                }
                sync_failures = 0;
            }
            Err(e) => {
                sync_failures += 1;
                error!("Periodic sync failed ({sync_failures}x): {e:#}");

                if sync_failures >= ALERT_THRESHOLD && !sync_alert_sent {
                    warn!(
                        "GitHub sync has failed {sync_failures} times in a row. Last error: {e}"
                    );
                    sync_alert_sent = true;
                }
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

            match pipelines::triage::run(&state.config, &state.db, &state.gh, &args, pipelines::triage::OutputFormat::Text, None)
                .await
            {
                Ok(()) => {
                    info!("Scheduled triage complete");

                    if triage_alert_sent {
                        warn!("Triage recovered after {} failures", triage_failures);
                        triage_alert_sent = false;
                    }
                    triage_failures = 0;
                }
                Err(e) => {
                    triage_failures += 1;
                    error!("Scheduled triage failed ({triage_failures}x): {e:#}");

                    if triage_failures >= ALERT_THRESHOLD && !triage_alert_sent {
                        warn!(
                            "AI triage has failed {triage_failures} times in a row. Last error: {e}"
                        );
                        triage_alert_sent = true;
                    }
                }
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

            match pipelines::triage::run(&state.config, &state.db, &state.gh, &args, pipelines::triage::OutputFormat::Text, None)
                .await
            {
                Ok(()) => info!("Scheduled retriage complete"),
                Err(e) => error!("Scheduled retriage failed: {e:#}"),
            }
        }

        // Cleanup old webhook events (keep 7 days)
        if let Err(e) = state.db.cleanup_old_events(7) {
            error!("Event cleanup failed: {e:#}");
        }

        // Auto-update check
        if state.config.update.enabled && last_update_check.elapsed() >= update_interval {
            last_update_check = Instant::now();
            update::auto_check_and_update().await;
        }

        // Daily notification recap (OSS: log only — Pro has full notify pipeline)
        if last_daily_notify.elapsed() >= daily_notify_interval {
            last_daily_notify = Instant::now();
            info!("Daily recap: {} repos active", 1);
            // TODO(pro): Pro notify pipeline sends full digest to Discord/Slack/email
        }
    }
}
