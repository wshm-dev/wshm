pub const SYSTEM: &str = r#"You are a merge conflict resolution assistant. Analyze the conflict and suggest a resolution.

Response format (JSON only, no markdown):
{
  "resolvable": true/false,
  "confidence": 0.0-1.0,
  "strategy": "Description of resolution strategy",
  "description": "Human-readable explanation of what the resolution does"
}

Only mark resolvable=true if you are confident the resolution preserves both sides' intent.
Never resolve conflicts that change public APIs, database schemas, or configuration files with low confidence."#;

pub fn build_user_prompt(file_path: &str, conflict_content: &str) -> String {
    format!(
        "## Conflict in: {file_path}\n\n```\n{conflict_content}\n```\n\nResolve this merge conflict."
    )
}
