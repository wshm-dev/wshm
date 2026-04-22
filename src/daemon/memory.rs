use tracing::{debug, info};

use crate::config::Config;

/// Store triage result in ICM for future context.
pub async fn store_triage(
    config: &Config,
    number: u64,
    category: &str,
    confidence: f64,
    summary: &str,
) {
    let topic = format!("{}:{}", config.daemon.icm_topic_prefix, config.repo_slug());
    let content = format!(
        "Issue #{} triaged: category={}, confidence={:.0}%, summary: {}",
        number,
        category,
        confidence * 100.0,
        summary
    );
    icm_store(&topic, &content, "medium").await;
}

/// Store PR analysis result in ICM.
pub async fn store_pr_analysis(
    config: &Config,
    number: u64,
    pr_type: &str,
    risk_level: &str,
    summary: &str,
) {
    let topic = format!("{}:{}", config.daemon.icm_topic_prefix, config.repo_slug());
    let content = format!(
        "PR #{} analyzed: type={}, risk={}, summary: {}",
        number, pr_type, risk_level, summary
    );
    icm_store(&topic, &content, "medium").await;
}

async fn icm_store(topic: &str, content: &str, importance: &str) {
    match tokio::process::Command::new("icm")
        .args(["store", "-t", topic, "-c", content, "-i", importance])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            info!("Stored to ICM topic={topic}");
        }
        Ok(output) => {
            debug!(
                "ICM store failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(e) => {
            debug!("ICM not available: {e}");
        }
    }
}
