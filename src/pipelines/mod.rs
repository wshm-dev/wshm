pub mod autogen;

/// Truncate a string to `max` chars, appending "…" if truncated.
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
pub mod changelog;
pub mod conflict_resolution;
pub mod dashboard;
pub mod merge_queue;
pub mod pr_analysis;
pub mod pr_health;
pub mod report;
pub mod review;
pub mod status;
pub mod triage;
