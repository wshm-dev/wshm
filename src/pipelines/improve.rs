//! `wshm improve` — AI-powered improvement loop.
//!
//! Analyzes the codebase and proposes feature/improvement issues.
//! With `--apply`, creates the issues on GitHub directly.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::ai::backend::AiBackend;
use crate::cli::ImproveArgs;
use crate::config::Config;
use crate::db::Database;
use crate::github::Client as GhClient;

const SYSTEM_PROMPT: &str = r#"You are a senior software engineer reviewing a codebase for improvements.
Analyze the provided code context and suggest concrete, actionable improvements.

For each suggestion, provide a JSON object. Respond with a JSON array of suggestions.

Categories:
- "refactor" — code quality, readability, maintainability
- "performance" — speed, memory, efficiency
- "testing" — missing tests, test coverage gaps
- "security" — potential vulnerabilities, hardening
- "feature" — small feature additions that would improve UX
- "docs" — documentation gaps

Rules:
- Each suggestion must be specific and actionable (not vague like "improve error handling")
- Include the relevant file paths
- Estimate effort: "trivial" (< 1h), "small" (1-4h), "medium" (4-16h)
- Maximum 10 suggestions, ordered by impact
- Focus on high-value, low-effort improvements first
- Do NOT suggest changes that are purely cosmetic

Response format (JSON array, no markdown):
[
  {
    "title": "Short descriptive title for the issue",
    "body": "Detailed description with context, rationale, and suggested approach",
    "category": "refactor|performance|testing|security|feature|docs",
    "effort": "trivial|small|medium",
    "files": ["src/relevant_file.rs"],
    "labels": ["enhancement", "good first issue"]
  }
]"#;

#[derive(Debug, Serialize, Deserialize)]
struct Suggestion {
    title: String,
    body: String,
    category: String,
    effort: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    labels: Vec<String>,
}

#[derive(Serialize)]
struct ImproveOutput {
    suggestions: Vec<SuggestionResult>,
}

#[derive(Serialize)]
struct SuggestionResult {
    title: String,
    category: String,
    effort: String,
    issue_number: Option<u64>,
    applied: bool,
}

pub async fn run(
    config: &Config,
    db: &Database,
    gh: &GhClient,
    args: &ImproveArgs,
    json: bool,
) -> Result<()> {
    let model = config.model_for("improve");
    let backend = AiBackend::from_config(config, model)?;

    // Build context: repo structure + open issues + recent triages
    let context = build_context(config, db)?;

    info!("Analyzing codebase for improvements...");

    let suggestions: Vec<Suggestion> = backend.complete(SYSTEM_PROMPT, &context).await?;

    if suggestions.is_empty() {
        if json {
            println!("{{\"suggestions\":[]}}");
        } else {
            println!("No improvements suggested.");
        }
        return Ok(());
    }

    let limit = args.limit.unwrap_or(5) as usize;
    let suggestions: Vec<Suggestion> = suggestions.into_iter().take(limit).collect();

    let mut results: Vec<SuggestionResult> = Vec::new();

    for suggestion in &suggestions {
        if !json {
            println!(
                "[{}] ({}) {} — {}",
                suggestion.category,
                suggestion.effort,
                suggestion.title,
                if args.apply { "CREATING" } else { "DRY-RUN" }
            );
        }

        let issue_number = if args.apply {
            let body = format!(
                "{}## 💡 Suggested Improvement\n\n\
                 **Category:** {} | **Effort:** {}\n\n\
                 {}\n\n\
                 {}\n\n\
                 {}\n\n{}",
                config.branding.header(),
                suggestion.category,
                suggestion.effort,
                suggestion.body,
                if suggestion.files.is_empty() {
                    String::new()
                } else {
                    format!(
                        "### Relevant files\n{}",
                        suggestion.files
                            .iter()
                            .map(|f| format!("- `{f}`"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                },
                if args.auto_fix {
                    "> 🔧 Auto-fix will be attempted on this issue.".to_string()
                } else {
                    "> Use `/wshm fix` to auto-generate a PR for this improvement.".to_string()
                },
                config.branding.footer("Suggested"),
            );

            let mut labels = suggestion.labels.clone();
            let category_label = match suggestion.category.as_str() {
                "feature" => "enhancement",
                "security" => "security",
                "docs" => "documentation",
                "testing" => "testing",
                _ => "enhancement",
            };
            if !labels.contains(&category_label.to_string()) {
                labels.push(category_label.to_string());
            }
            let effort_label = format!("effort:{}", suggestion.effort);
            if !labels.contains(&effort_label) {
                labels.push(effort_label);
            }

            let labels = config.filter_labels(labels);

            match gh
                .create_issue(&suggestion.title, &body, &labels)
                .await
            {
                Ok(num) => {
                    info!("Created issue #{num}: {}", suggestion.title);
                    if !json {
                        println!("  → Created issue #{num}");
                    }
                    Some(num)
                }
                Err(e) => {
                    tracing::error!("Failed to create issue: {e:#}");
                    None
                }
            }
        } else {
            if !json {
                println!("  {}", suggestion.body.lines().next().unwrap_or(""));
                if !suggestion.files.is_empty() {
                    println!(
                        "  Files: {}",
                        suggestion.files.join(", ")
                    );
                }
            }
            None
        };

        results.push(SuggestionResult {
            title: suggestion.title.clone(),
            category: suggestion.category.clone(),
            effort: suggestion.effort.clone(),
            issue_number,
            applied: args.apply,
        });
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ImproveOutput {
                suggestions: results
            })?
        );
    } else if args.apply && args.auto_fix {
        println!("\nIssues created. Auto-fix will be triggered by the daemon on next poll.");
    }

    Ok(())
}

fn build_context(config: &Config, db: &Database) -> Result<String> {
    let mut ctx = format!(
        "## Repository: {}\n\n",
        config.repo_slug()
    );

    // Add open issues to avoid duplicates
    let open_issues = db.get_open_issues()?;
    if !open_issues.is_empty() {
        ctx.push_str("## Existing open issues (do NOT suggest duplicates):\n");
        for issue in open_issues.iter().take(30) {
            ctx.push_str(&format!("- #{}: {}\n", issue.number, issue.title));
        }
        ctx.push('\n');
    }

    // Add file tree (from git ls-files)
    let ls_output = std::process::Command::new("git")
        .args(["ls-files"])
        .output();
    if let Ok(output) = ls_output {
        let files = String::from_utf8_lossy(&output.stdout);
        let file_list: Vec<&str> = files.lines().collect();
        ctx.push_str(&format!("## Project files ({} total):\n```\n", file_list.len()));
        // Show all files up to 200, then truncate
        for f in file_list.iter().take(200) {
            ctx.push_str(&format!("{f}\n"));
        }
        if file_list.len() > 200 {
            ctx.push_str(&format!("... and {} more files\n", file_list.len() - 200));
        }
        ctx.push_str("```\n\n");
    }

    // Add key source files content (README, main entry, config)
    for candidate in &["README.md", "CLAUDE.md", "Cargo.toml", "package.json", "pyproject.toml"] {
        if let Ok(content) = std::fs::read_to_string(candidate) {
            let truncated = if content.len() > 3000 {
                format!("{}...\n(truncated)", &content[..3000])
            } else {
                content
            };
            ctx.push_str(&format!("## {candidate}\n```\n{truncated}\n```\n\n"));
        }
    }

    Ok(ctx)
}
