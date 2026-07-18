pub mod backup;
pub mod context;
pub mod migrate;
pub mod revert;

/// Truncate a string to `max` chars, appending "…" if truncated.
/// Uses char boundaries to avoid panics on multi-byte UTF-8 input.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end = s
            .char_indices()
            .nth(max - 1)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}…", &s[..end])
    }
}

/// Extract issue numbers linked via "fixes #N", "closes #N", "resolves #N" patterns.
pub fn extract_linked_issue_numbers(body: &str) -> std::collections::HashSet<u64> {
    // Safe: regex pattern is hardcoded and always valid
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:fix(?:es)?|close[sd]?|resolve[sd]?)\s+#(\d+)").unwrap()
    });
    RE.captures_iter(body)
        .filter_map(|cap| cap[1].parse().ok())
        .collect()
}

/// Extract issue links with their type ("fixes", "closes", "resolves") + number.
pub fn extract_linked_issues_with_type(body: &str) -> Vec<(String, u64)> {
    // Safe: regex pattern is hardcoded and always valid
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(fix(?:es)?|close[sd]?|resolve[sd]?)\s+#(\d+)").unwrap()
    });
    let mut seen = std::collections::HashSet::new();
    RE.captures_iter(body)
        .filter_map(|cap| {
            let link_type = cap[1].to_lowercase();
            let num: u64 = cap[2].parse().ok()?;
            if seen.insert(num) {
                Some((link_type, num))
            } else {
                None
            }
        })
        .collect()
}
/// Whether an AI backend error is a usage/rate limit (vs. a per-item failure).
///
/// When the AI provider is rate limited, every remaining item in a batch will
/// fail the same way, so callers use this to abort the batch early instead of
/// burning one failed request per item. Matches the distinct marker emitted by
/// the claude CLI backend (see `ai::client`).
pub fn is_usage_limit_error(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}").to_ascii_lowercase();
    msg.contains("usage limit") || msg.contains("rate limit")
}

pub mod merge_queue;
pub mod pr_analysis;
pub mod pr_health;
pub mod status;
pub mod triage;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_usage_limit_error_detects_limit() {
        let e = anyhow::anyhow!("claude CLI usage limit reached: You've hit your limit");
        assert!(is_usage_limit_error(&e));
        let e = anyhow::anyhow!("HTTP 429: rate limit exceeded");
        assert!(is_usage_limit_error(&e));
    }

    #[test]
    fn test_is_usage_limit_error_ignores_other_failures() {
        let e = anyhow::anyhow!("claude CLI failed (exit 1): no output");
        assert!(!is_usage_limit_error(&e));
        let e = anyhow::anyhow!("Failed to parse AI response as JSON");
        assert!(!is_usage_limit_error(&e));
    }
}
