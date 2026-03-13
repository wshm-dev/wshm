use wshm::ai::prompts::inline_review;
use wshm::ai::schemas::{InlineComment, InlineReviewResult};
use wshm::pipelines::review::compute_diff_size;

const SAMPLE_DIFF: &str = r#"diff --git a/src/main.rs b/src/main.rs
index abc1234..def5678 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,5 +1,6 @@
 fn main() {
-    println!("hello");
+    println!("hello world");
+    println!("new line");
 }
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -10,3 +10,5 @@
 pub fn existing() {}
+
+pub fn new_function() -> bool { true }
diff --git a/Cargo.lock b/Cargo.lock
index aaa..bbb 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1,3 +1,4 @@
 [root]
+name = "test"
"#;

#[test]
fn test_split_diff_by_file() {
    let files = inline_review::split_diff_by_file(SAMPLE_DIFF);

    assert_eq!(files.len(), 3);
    assert_eq!(files[0].0, "src/main.rs");
    assert_eq!(files[1].0, "src/lib.rs");
    assert_eq!(files[2].0, "Cargo.lock");

    // Each chunk should contain the diff header
    assert!(files[0].1.contains("diff --git"));
    assert!(files[0].1.contains("hello world"));
    assert!(files[1].1.contains("new_function"));
}

#[test]
fn test_split_diff_empty() {
    let files = inline_review::split_diff_by_file("");
    assert!(files.is_empty());
}

#[test]
fn test_compute_diff_size() {
    let size = compute_diff_size(SAMPLE_DIFF);

    assert_eq!(size.additions, 5); // 2 in main.rs + 2 in lib.rs + 1 in Cargo.lock
    assert_eq!(size.deletions, 1); // 1 - line in main.rs
    assert_eq!(size.files_changed, 3);
    assert_eq!(size.total_lines(), 6);
    assert!(!size.is_large());
    assert!(!size.is_huge());
}

#[test]
fn test_build_file_prompt() {
    let prompt = inline_review::build_file_prompt(
        "Fix memory leak",
        "This fixes the pool issue",
        "src/pool.rs",
        "+    drop(conn);",
    );

    assert!(prompt.contains("Fix memory leak"));
    assert!(prompt.contains("src/pool.rs"));
    assert!(prompt.contains("drop(conn)"));
    assert!(prompt.contains("Review ONLY this file"));
}

#[test]
fn test_inline_comment_deserialization() {
    let json = r#"{
        "path": "src/main.rs",
        "line": 42,
        "body": "Potential null deref",
        "severity": "error",
        "category": "bug",
        "suggestion": "if let Some(x) = val { x } else { return; }"
    }"#;

    let comment: InlineComment = serde_json::from_str(json).unwrap();
    assert_eq!(comment.path, "src/main.rs");
    assert_eq!(comment.line, 42);
    assert_eq!(comment.severity, "error");
    assert_eq!(comment.category, "bug");
    assert!(comment.suggestion.is_some());
}

#[test]
fn test_inline_comment_defaults() {
    let json = r#"{
        "path": "src/main.rs",
        "line": 10,
        "body": "Minor issue"
    }"#;

    let comment: InlineComment = serde_json::from_str(json).unwrap();
    assert_eq!(comment.severity, "warning");
    assert_eq!(comment.category, "logic");
    assert!(comment.suggestion.is_none());
}

#[test]
fn test_review_result_deserialization() {
    let json = r#"{
        "comments": [
            {
                "path": "src/api.rs",
                "line": 15,
                "body": "SQL injection risk",
                "severity": "error",
                "category": "security",
                "suggestion": "conn.execute(\"SELECT * FROM t WHERE id = ?\", &[&id])"
            }
        ],
        "summary": "Found 1 security issue",
        "stats": {"errors": 1, "warnings": 0, "infos": 0}
    }"#;

    let result: InlineReviewResult = serde_json::from_str(json).unwrap();
    assert_eq!(result.comments.len(), 1);
    assert_eq!(result.stats.errors, 1);
    assert_eq!(result.comments[0].category, "security");
}

#[test]
fn test_empty_review_result() {
    let json = r#"{
        "comments": [],
        "summary": "No issues found.",
        "stats": {"errors": 0, "warnings": 0, "infos": 0}
    }"#;

    let result: InlineReviewResult = serde_json::from_str(json).unwrap();
    assert!(result.comments.is_empty());
    assert_eq!(result.summary, "No issues found.");
}
