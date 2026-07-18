use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

use crate::cli::TriageArgs;
use crate::github;
use crate::pipelines;

use super::DaemonState;

/// Number of consecutive failures before sending an alert.
const ALERT_THRESHOLD: u32 = 3;

/// Process-global last-auto-update timestamp (unix seconds). Per-repo
/// schedulers all check the same atomic before triggering
/// `run_auto_update()` so 50 repos don't race to download the same
/// release artifact and clobber each other on the file replace.
static LAST_AUTO_UPDATE_AT: AtomicI64 = AtomicI64::new(0);

/// Try to claim the next auto-update slot. Returns true exactly once per
/// `interval` across all callers; returns false otherwise.
fn try_claim_auto_update(interval: Duration) -> bool {
    let now = chrono::Utc::now().timestamp();
    let interval_secs = interval.as_secs() as i64;
    loop {
        let prev = LAST_AUTO_UPDATE_AT.load(Ordering::Acquire);
        if prev != 0 && now.saturating_sub(prev) < interval_secs {
            return false;
        }
        if LAST_AUTO_UPDATE_AT
            .compare_exchange(prev, now, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
        // Lost the race — another scheduler claimed the slot; retry the
        // freshness check, which will now report "too soon".
    }
}

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
    // Log the dry-run skip notice only once instead of every cycle.
    let mut dry_run_triage_logged = false;

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
        crate::pro_hooks::run_auto_update().await;
    }

    loop {
        tokio::time::sleep(interval).await;

        let features = state.features();
        if !features.collect_issues && !features.collect_prs {
            info!(
                "[{}] collect_issues and collect_prs both disabled — skipping periodic sync",
                state.config.repo_slug()
            );
            continue;
        }
        info!("Periodic sync triggered (incremental)");
        match github::sync::incremental_sync_full(&state.gh(), state.db.as_ref()).await {
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
                    warn!("GitHub sync has failed {sync_failures} times in a row. Last error: {e}");
                    sync_alert_sent = true;
                }
                continue;
            }
        }

        // Triage untriaged issues after sync. Both the legacy
        // [triage].enabled config flag AND the per-repo features.triage_issues
        // toggle must be true.
        //
        // The scheduled batch only runs when applying: in dry-run nothing is
        // persisted, so get_issues_needing_triage() returns the same issues
        // every cycle — an infinite re-triage loop that burns AI quota for no
        // visible effect. Webhook-driven triage still works in dry-run for
        // one-off previews.
        if state.config.triage.enabled && features.triage_issues && !state.apply() {
            if !dry_run_triage_logged {
                warn!(
                    "Scheduled triage skipped in dry-run mode (apply=false): it would \
                     re-triage the same issues every cycle and waste AI quota. Set \
                     apply=true to enable applied triage."
                );
                dry_run_triage_logged = true;
            }
        } else if state.config.triage.enabled && features.triage_issues {
            let args = TriageArgs {
                issue: None,
                apply: state.apply(),
                retriage: false,
            };

            match pipelines::triage::run_with_filters(
                &state.config,
                state.db.as_ref(),
                &state.gh(),
                &args,
                pipelines::triage::OutputFormat::Text,
                None,
                Some(&features.filters),
            )
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

        // Periodic retriage: re-evaluate stale triage results. Same dual
        // gate as the regular triage loop above, plus apply (retriage only
        // makes sense once results are persisted, which requires apply).
        if state.config.triage.enabled
            && features.triage_issues
            && state.apply()
            && retriage_interval_hours > 0
            && last_retriage.elapsed() >= retriage_interval
        {
            last_retriage = Instant::now();
            info!("Periodic retriage triggered (interval: {retriage_interval_hours}h)");

            let args = TriageArgs {
                issue: None,
                apply: state.apply(),
                retriage: true,
            };

            match pipelines::triage::run_with_filters(
                &state.config,
                state.db.as_ref(),
                &state.gh(),
                &args,
                pipelines::triage::OutputFormat::Text,
                None,
                Some(&features.filters),
            )
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

        // Auto-update check — gated on a process-global atomic so only
        // one repo's scheduler triggers the download per interval, even
        // when N>1 schedulers race here at the same tick.
        if state.config.update.enabled
            && last_update_check.elapsed() >= update_interval
            && try_claim_auto_update(update_interval)
        {
            last_update_check = Instant::now();
            crate::pro_hooks::run_auto_update().await;
        }

        // Daily notification recap (OSS: log only — Pro has full notify pipeline)
        if last_daily_notify.elapsed() >= daily_notify_interval {
            last_daily_notify = Instant::now();
            info!("Daily recap: {} repos active", 1);
            // TODO(pro): Pro notify pipeline sends full digest to Discord/Slack/email
        }
    }
}
