//! ICM (Infinite Context Memory) integration.
//!
//! Calls the `icm` CLI to recall/store memories.
//! Works with any AI provider — ICM is called at the wshm level,
//! not inside the AI tool.

use tracing::{debug, warn};

/// Check if `icm` CLI is available on the system.
pub fn is_available() -> bool {
    std::process::Command::new("icm")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Recall relevant context from ICM, formatted for prompt injection.
/// Returns empty string if ICM is not available or recall fails.
pub fn recall_context(query: &str, limit: usize) -> String {
    let output = std::process::Command::new("icm")
        .args(["recall-context", query, "--limit", &limit.to_string()])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let ctx = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if ctx.is_empty() {
                debug!("ICM recall returned no results for: {query}");
            } else {
                debug!("ICM recalled {} bytes of context", ctx.len());
            }
            ctx
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            debug!("ICM recall failed: {err}");
            String::new()
        }
        Err(e) => {
            debug!("ICM not available: {e}");
            String::new()
        }
    }
}

/// Store a memory in ICM.
pub fn store(topic: &str, content: &str, importance: &str, keywords: &[&str]) {
    let mut cmd = std::process::Command::new("icm");
    cmd.args(["store", "-t", topic, "-c", content, "-i", importance]);

    if !keywords.is_empty() {
        cmd.args(["-k", &keywords.join(",")]);
    }

    match cmd.output() {
        Ok(o) if o.status.success() => {
            debug!("ICM stored memory in topic '{topic}'");
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            warn!("ICM store failed: {err}");
        }
        Err(e) => {
            debug!("ICM not available for store: {e}");
        }
    }
}

/// Recall raw memories (JSON) for programmatic use.
pub fn recall(query: &str, topic: Option<&str>, limit: usize) -> Vec<String> {
    let mut cmd = std::process::Command::new("icm");
    cmd.args(["recall", query, "--limit", &limit.to_string()]);

    if let Some(t) = topic {
        cmd.args(["--topic", t]);
    }

    match cmd.output() {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            text.lines().map(|l| l.to_string()).collect()
        }
        _ => Vec::new(),
    }
}
