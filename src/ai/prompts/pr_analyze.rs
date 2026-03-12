use crate::db::pulls::PullRequest;

pub const SYSTEM: &str = r#"You are a pull request analysis assistant. Analyze the given PR and respond with a JSON object.

Risk levels: "low", "medium", "high"
PR types: "bug-fix", "feature", "refactor", "docs", "chore"

Response format (JSON only, no markdown):
{
  "summary": "2-3 sentence description of what this PR does",
  "risk_level": "low|medium|high",
  "pr_type": "bug-fix|feature|refactor|docs|chore",
  "linked_issues": [1, 2],
  "review_checklist": {
    "tests_present": true/false,
    "breaking_change": true/false,
    "docs_updated": true/false
  },
  "suggested_labels": ["label1", "label2"]
}

Detect linked issues from patterns like "fixes #X", "closes #X", "resolves #X" in the PR body."#;

pub fn build_user_prompt(pr: &PullRequest, diff: Option<&str>) -> String {
    let mut prompt = format!(
        "## PR #{}: {}\n\n{}\n\n**Author:** {}\n**Base:** {} ← **Head:** {}\n**Labels:** {}\n",
        pr.number,
        pr.title,
        pr.body.as_deref().unwrap_or("(no description)"),
        pr.author.as_deref().unwrap_or("unknown"),
        pr.base_ref.as_deref().unwrap_or("unknown"),
        pr.head_ref.as_deref().unwrap_or("unknown"),
        if pr.labels.is_empty() {
            "none".to_string()
        } else {
            pr.labels.join(", ")
        },
    );

    if let Some(diff) = diff {
        // Truncate very large diffs
        let truncated = if diff.len() > 10000 {
            format!(
                "{}...\n(truncated, {} total bytes)",
                &diff[..10000],
                diff.len()
            )
        } else {
            diff.to_string()
        };
        prompt.push_str(&format!("\n## Diff:\n```\n{truncated}\n```\n"));
    }

    prompt
}
