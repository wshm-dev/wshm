pub const SYSTEM: &str = r#"You are an expert code reviewer. Analyze the diff and find real issues: bugs, security vulnerabilities, performance problems, race conditions, resource leaks, or logic errors.

DO NOT comment on:
- Style, formatting, naming conventions
- Missing documentation or comments
- Trivial issues or nitpicks
- Things that are correct but could be done differently
- Import ordering or whitespace

For each issue, provide a category and, when possible, a concrete code suggestion using GitHub's suggestion syntax.

Response format (JSON only, no markdown wrapping):
{
  "comments": [
    {
      "path": "src/file.rs",
      "line": 42,
      "body": "Concise description of the issue and why it matters",
      "severity": "error|warning|info",
      "category": "bug|security|perf|race-condition|resource-leak|logic|error-handling",
      "suggestion": "the corrected line(s) of code, or null if no fix is obvious"
    }
  ],
  "summary": "1-3 sentence overview of the review findings",
  "stats": {"errors": 0, "warnings": 0, "infos": 0}
}

Rules:
- Only flag things you are CONFIDENT are actual problems
- "error" = will cause bugs/crashes/vulnerabilities in production
- "warning" = likely problem, should be fixed before merge
- "info" = potential improvement, non-blocking
- If no real issues exist, return {"comments": [], "summary": "No issues found.", "stats": {"errors": 0, "warnings": 0, "infos": 0}}
- The "line" field must reference a line number in the NEW file (lines starting with +)
- The "suggestion" field should contain ONLY the replacement code (no ``` markers), or null
- Max 15 comments per file chunk — focus on the most impactful issues"#;

/// Build prompt for a single file chunk (preferred: better context per file).
pub fn build_file_prompt(
    pr_title: &str,
    pr_body: &str,
    file_path: &str,
    file_diff: &str,
) -> String {
    let truncated = if file_diff.len() > 15000 {
        format!(
            "{}...\n(truncated, {} total bytes)",
            &file_diff[..15000],
            file_diff.len()
        )
    } else {
        file_diff.to_string()
    };

    format!(
        "## PR: {pr_title}\n\n{pr_body}\n\n## File: {file_path}\n\n```diff\n{truncated}\n```\n\n\
         Review ONLY this file. Use the exact path \"{file_path}\" in your comments."
    )
}

/// Build prompt for the full diff (fallback for small PRs or single-file changes).
pub fn build_user_prompt(pr_title: &str, pr_body: &str, diff: &str) -> String {
    let truncated = if diff.len() > 30000 {
        format!(
            "{}...\n(truncated, {} total bytes)",
            &diff[..30000],
            diff.len()
        )
    } else {
        diff.to_string()
    };

    format!("## PR: {pr_title}\n\n{pr_body}\n\n## Diff:\n```diff\n{truncated}\n```")
}

/// Split a unified diff into per-file chunks: Vec<(file_path, file_diff)>.
pub fn split_diff_by_file(diff: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut current_path = String::new();
    let mut current_chunk = String::new();

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            // Flush previous file
            if !current_path.is_empty() && !current_chunk.is_empty() {
                files.push((current_path.clone(), current_chunk.clone()));
            }
            current_chunk.clear();

            // Extract path from "diff --git a/path b/path"
            current_path = line.split(" b/").last().unwrap_or("unknown").to_string();
        }
        current_chunk.push_str(line);
        current_chunk.push('\n');
    }

    // Flush last file
    if !current_path.is_empty() && !current_chunk.is_empty() {
        files.push((current_path, current_chunk));
    }

    files
}
