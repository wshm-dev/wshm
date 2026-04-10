pub mod conflict_resolve;
pub mod inline_review;
pub mod issue_classify;
pub mod issue_fix;
pub mod pr_analyze;

/// Find the largest byte offset <= `max_bytes` that falls on a UTF-8 char boundary.
pub fn truncate_utf8(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}
