use crate::ai::schemas::ReviewWalkthrough;

pub const SYSTEM: &str = r#"You summarize a pull request for its reviewers. Do NOT look for bugs: describe what the change does.

Response format (JSON only, no markdown wrapping):
{
  "overview": "2-3 sentences: what the PR changes and why, in plain words",
  "files": [
    {"path": "src/file.rs", "change": "one sentence: what changed in this file"}
  ],
  "diagram": "sequenceDiagram\n    participant A\n    ..."
}

Rules:
- "files": one entry per changed file, using the exact path from the diff. Group trivial files (lockfiles, generated code) in a single entry if there are many.
- "diagram": a Mermaid sequenceDiagram of the runtime flow the PR adds or modifies (who calls whom, in order). Mermaid source only, no ``` fences.
- Keep the diagram small: at most 8 participants and 15 messages. Participant names must be identifiers or quoted labels taken from the code.
- Set "diagram" to null when the change has no runtime flow worth drawing (docs, config, renames, pure refactors, tests only).
- Do not invent components that are not in the diff."#;

/// Max diff bytes sent to the walkthrough pass (whole PR, single call).
const MAX_DIFF_BYTES: usize = 40_000;
/// GitHub renders very long Mermaid blocks poorly; a model that ignored the
/// size rule gets its diagram dropped rather than truncated mid-statement.
const MAX_DIAGRAM_BYTES: usize = 4_000;
const MAX_FILE_ROWS: usize = 50;

pub fn build_user_prompt(pr_title: &str, pr_body: &str, diff: &str) -> String {
    let truncated = if diff.len() > MAX_DIFF_BYTES {
        let end = super::truncate_utf8(diff, MAX_DIFF_BYTES);
        format!(
            "{}...\n(truncated, {} total bytes)",
            &diff[..end],
            diff.len()
        )
    } else {
        diff.to_string()
    };

    let safe_title = super::issue_classify::sanitize_user_content(pr_title);
    let safe_body = super::issue_classify::sanitize_user_content(
        &super::issue_classify::truncate_body(pr_body, 4000),
    );
    format!("## PR: {safe_title}\n\n{safe_body}\n\n## Diff:\n```diff\n{truncated}\n```")
}

/// Return the diagram as safe Mermaid source, or None if it should not be
/// rendered (empty, oversized, or not a diagram GitHub knows how to draw).
pub fn sanitize_diagram(raw: &str) -> Option<String> {
    let mut src = raw.trim();
    if let Some(rest) = src.strip_prefix("```mermaid") {
        src = rest;
    } else if let Some(rest) = src.strip_prefix("```") {
        src = rest;
    }
    let src = src.trim().strip_suffix("```").unwrap_or(src).trim();

    if src.is_empty() || src.len() > MAX_DIAGRAM_BYTES || src.contains("```") {
        return None;
    }
    let first = src.lines().next().unwrap_or("").trim();
    let known = ["sequenceDiagram", "flowchart", "graph "];
    if !known.iter().any(|k| first.starts_with(k)) {
        return None;
    }
    Some(src.to_string())
}

fn table_cell(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
        .replace('|', "\\|")
        .trim()
        .to_string()
}

/// Render the walkthrough as GitHub-flavored Markdown for the review body.
/// Returns an empty string when there is nothing to show.
pub fn to_markdown(w: &ReviewWalkthrough) -> String {
    let mut out = String::new();

    let overview = w.overview.trim();
    if !overview.is_empty() {
        out.push_str(&crate::pipelines::truncate(overview, 800));
        out.push_str("\n\n");
    }

    let rows: Vec<&crate::ai::schemas::WalkthroughFile> = w
        .files
        .iter()
        .filter(|f| !f.path.trim().is_empty())
        .collect();
    if !rows.is_empty() {
        out.push_str("| File | Change |\n|---|---|\n");
        for f in rows.iter().take(MAX_FILE_ROWS) {
            let path = table_cell(&f.path).replace('`', "");
            let change = crate::pipelines::truncate(&table_cell(&f.change), 240);
            out.push_str(&format!("| `{path}` | {change} |\n"));
        }
        if rows.len() > MAX_FILE_ROWS {
            out.push_str(&format!(
                "\n*…and {} more files.*\n",
                rows.len() - MAX_FILE_ROWS
            ));
        }
        out.push('\n');
    }

    if let Some(diagram) = w.diagram.as_deref().and_then(sanitize_diagram) {
        out.push_str("```mermaid\n");
        out.push_str(&diagram);
        out.push_str("\n```\n");
    }

    if out.trim().is_empty() {
        return String::new();
    }
    format!("### Walkthrough\n\n{}", out.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::schemas::WalkthroughFile;

    #[test]
    fn sanitize_strips_fences_and_keeps_known_diagrams() {
        let d = sanitize_diagram("```mermaid\nsequenceDiagram\n  A->>B: hi\n```").unwrap();
        assert_eq!(d, "sequenceDiagram\n  A->>B: hi");
        assert!(sanitize_diagram("flowchart LR\n A-->B").is_some());
    }

    #[test]
    fn sanitize_rejects_empty_unknown_oversized_or_nested_fences() {
        assert!(sanitize_diagram("").is_none());
        assert!(sanitize_diagram("   ").is_none());
        assert!(sanitize_diagram("Here is the diagram: A->>B").is_none());
        assert!(sanitize_diagram("sequenceDiagram\n```\nrm -rf\n```").is_none());
        let big = format!("sequenceDiagram\n{}", "  A->>B: x\n".repeat(1000));
        assert!(sanitize_diagram(&big).is_none());
    }

    #[test]
    fn markdown_escapes_table_cells_and_includes_diagram() {
        let w = ReviewWalkthrough {
            overview: "Adds a cache.".into(),
            files: vec![WalkthroughFile {
                path: "src/a|b.rs".into(),
                change: "new\nfn | thing".into(),
            }],
            diagram: Some("sequenceDiagram\n  A->>B: get".into()),
        };
        let md = to_markdown(&w);
        assert!(md.starts_with("### Walkthrough\n\nAdds a cache."));
        assert!(md.contains("| `src/a\\|b.rs` | new fn \\| thing |"));
        assert!(md.contains("```mermaid\nsequenceDiagram\n  A->>B: get\n```"));
    }

    #[test]
    fn markdown_is_empty_when_nothing_usable() {
        let w = ReviewWalkthrough {
            overview: "  ".into(),
            files: vec![],
            diagram: Some("not a diagram".into()),
        };
        assert_eq!(to_markdown(&w), "");
    }

    #[test]
    fn walkthrough_parses_null_fields() {
        let w: ReviewWalkthrough =
            serde_json::from_str(r#"{"overview":null,"files":null,"diagram":null}"#).unwrap();
        assert!(w.files.is_empty() && w.diagram.is_none());
    }
}
