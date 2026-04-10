pub mod client;
pub mod git;
pub mod issues;
pub mod pulls;
pub mod sync;

pub use client::Client;

/// Maximum items per page for GitHub API pagination.
pub const GITHUB_PER_PAGE: u32 = 100;

/// Extract label names from a GitHub API JSON object.
pub fn extract_labels(json: &serde_json::Value) -> Vec<String> {
    json.get("labels")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l.get("name").and_then(|n| n.as_str()))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Extract author login from a GitHub API JSON object.
pub fn extract_author(json: &serde_json::Value) -> Option<String> {
    json.get("user")
        .and_then(|u| u.get("login"))
        .and_then(|v| v.as_str())
        .map(String::from)
}
