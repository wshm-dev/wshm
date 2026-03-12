use crate::db::issues::Issue;

pub const SYSTEM: &str = r#"You are a code fix assistant. Generate a minimal fix for the described bug.

Response format (JSON only, no markdown):
{
  "files": [
    {
      "path": "src/example.rs",
      "action": "modify",
      "diff": "unified diff content"
    }
  ],
  "explanation": "Brief explanation of the fix",
  "confidence": 0.0-1.0
}

Rules:
- Only fix the specific bug described
- Touch the minimum number of files (1-3 max)
- Never change unrelated code
- Never change public APIs unless the bug IS the API
- Include test updates if existing tests need to change"#;

pub fn build_user_prompt(issue: &Issue, relevant_code: &[(String, String)]) -> String {
    let mut prompt = format!(
        "## Bug Report (Issue #{}):\n**{}**\n\n{}\n",
        issue.number,
        issue.title,
        issue.body.as_deref().unwrap_or("(no description)"),
    );

    if !relevant_code.is_empty() {
        prompt.push_str("\n## Relevant source files:\n");
        for (path, content) in relevant_code {
            prompt.push_str(&format!("\n### {path}\n```\n{content}\n```\n"));
        }
    }

    prompt
}
