use anyhow::Result;
use tracing::info;

use crate::db::Database;
use crate::github::Client as GhClient;

/// Revert all wshm actions: remove comments, remove wshm-applied labels, clear triage/analysis results.
pub async fn run(db: &Database, gh: &GhClient, apply: bool) -> Result<()> {
    let open_issues = db.get_open_issues()?;
    let open_pulls = db.get_open_pulls()?;

    let mut comment_count = 0u64;
    let mut label_count = 0u64;

    // Process issues
    for issue in &open_issues {
        // Find and remove wshm comment
        if let Ok(Some(comment_id)) = gh.find_wshm_comment(issue.number, &gh.comment_marker).await {
            if apply {
                info!("Deleting wshm comment {comment_id} on issue #{}", issue.number);
                gh.delete_comment(comment_id).await?;
            } else {
                println!("[DRY-RUN] Would delete wshm comment on issue #{}", issue.number);
            }
            comment_count += 1;
        }

        // Remove wshm-applied labels
        let wshm_labels = db.get_wshm_applied_labels(issue.number)?;
        for label in &wshm_labels {
            if apply {
                info!("Removing label '{}' from issue #{}", label, issue.number);
                if let Err(e) = gh.remove_label(issue.number, label).await {
                    tracing::warn!("Failed to remove label '{label}' from #{}: {e}", issue.number);
                }
            } else {
                println!("[DRY-RUN] Would remove label '{}' from issue #{}", label, issue.number);
            }
            label_count += 1;
        }
    }

    // Process PRs
    for pr in &open_pulls {
        if let Ok(Some(comment_id)) = gh.find_wshm_comment(pr.number, &gh.comment_marker).await {
            if apply {
                info!("Deleting wshm comment {comment_id} on PR #{}", pr.number);
                gh.delete_comment(comment_id).await?;
            } else {
                println!("[DRY-RUN] Would delete wshm comment on PR #{}", pr.number);
            }
            comment_count += 1;
        }
    }

    // Clear DB tables
    if apply {
        db.with_conn(|conn| {
            conn.execute("DELETE FROM triage_results", [])?;
            conn.execute("DELETE FROM pr_analyses", [])?;
            Ok(())
        })?;
        info!("Cleared triage_results and pr_analyses tables.");
    }

    if apply {
        println!("Reverted: {} comments deleted, {} labels removed.", comment_count, label_count);
    } else {
        println!(
            "Dry-run: would delete {} comments, remove {} labels, clear triage_results + pr_analyses.",
            comment_count, label_count
        );
    }

    Ok(())
}
