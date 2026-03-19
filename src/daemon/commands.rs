//! Slash command parser and executor for interactive bot mode.
//!
//! Users can post comments on issues/PRs with `/wshm <command>` to trigger actions.
//!
//! Supported commands:
//!   /wshm triage         — (re)triage this issue
//!   /wshm analyze        — (re)analyze this PR
//!   /wshm review         — run inline code review on this PR
//!   /wshm label <name>   — add a label
//!   /wshm unlabel <name> — remove a label
//!   /wshm fix            — auto-generate a fix PR for this issue
//!   /wshm queue          — show merge queue position for this PR
//!   /wshm health         — check PR health (duplicates, staleness)
//!   /wshm help           — show available commands

use anyhow::Result;
use tracing::info;

use crate::cli::{FixArgs, PrArgs, ReviewArgs, TriageArgs};
use crate::config::Config;
use crate::db::Database;
use crate::github::sync as gh_sync;
use crate::github::Client as GhClient;
use crate::pipelines;

/// A parsed slash command from a comment body.
#[derive(Debug)]
pub enum SlashCommand {
    Triage,
    Analyze,
    Review,
    Label(String),
    Unlabel(String),
    Fix,
    Queue,
    Health,
    Help,
    Unknown(String),
}

/// Parse the first slash command from a comment body.
/// Uses the branding command_prefix (default: "/wshm").
/// Returns None if no command found.
pub fn parse(comment_body: &str, prefix: &str) -> Option<SlashCommand> {
    // Support both /wshm and @wshm (and custom prefix variants)
    let at_prefix = if let Some(name) = prefix.strip_prefix('/') {
        format!("@{name}")
    } else {
        format!("@{prefix}")
    };

    for line in comment_body.lines() {
        let trimmed = line.trim();
        let rest = trimmed
            .strip_prefix(prefix)
            .or_else(|| trimmed.strip_prefix(&at_prefix));
        if let Some(rest) = rest {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            let cmd = match parts.first().map(|s| s.to_lowercase()) {
                Some(ref c) if c == "triage" || c == "retriage" => SlashCommand::Triage,
                Some(ref c) if c == "analyze" || c == "analyse" || c == "reanalyze" => {
                    SlashCommand::Analyze
                }
                Some(ref c) if c == "review" => SlashCommand::Review,
                Some(ref c) if c == "label" || c == "add-label" => {
                    if let Some(label) = parts.get(1) {
                        SlashCommand::Label(label.to_string())
                    } else {
                        SlashCommand::Unknown("label requires a name".into())
                    }
                }
                Some(ref c) if c == "unlabel" || c == "remove-label" => {
                    if let Some(label) = parts.get(1) {
                        SlashCommand::Unlabel(label.to_string())
                    } else {
                        SlashCommand::Unknown("unlabel requires a name".into())
                    }
                }
                Some(ref c) if c == "fix" || c == "autofix" || c == "auto-fix" => SlashCommand::Fix,
                Some(ref c) if c == "queue" || c == "merge-queue" => SlashCommand::Queue,
                Some(ref c) if c == "health" || c == "check" => SlashCommand::Health,
                Some(ref c) if c == "help" => SlashCommand::Help,
                Some(other) => SlashCommand::Unknown(other),
                None => SlashCommand::Help, // bare "/wshm" = help
            };
            return Some(cmd);
        }
    }
    None
}

/// Execute a slash command and return a response comment body.
pub async fn execute(
    cmd: &SlashCommand,
    number: u64,
    is_pr: bool,
    config: &Config,
    db: &Database,
    gh: &GhClient,
    apply: bool,
    triggered_by: Option<&str>,
) -> Result<String> {
    match cmd {
        SlashCommand::Triage => {
            info!("Slash command: triage issue #{number}");
            gh_sync::incremental_sync(gh, db, "issues").await?;
            let args = TriageArgs {
                issue: Some(number),
                apply,
                retriage: false,
            };
            pipelines::triage::run(config, db, gh, &args, false, None).await?;
            Ok(format!(
                "Re-triaged issue #{number}. {}",
                if apply {
                    "Labels and comments updated."
                } else {
                    "(dry-run — use `--apply` on daemon to enable actions)"
                }
            ))
        }
        SlashCommand::Analyze => {
            if !is_pr {
                return Ok("This command only works on pull requests.".into());
            }
            info!("Slash command: analyze PR #{number}");
            gh_sync::incremental_sync(gh, db, "pulls").await?;
            let args = PrArgs {
                pr: Some(number),
                apply,
            };
            pipelines::pr_analysis::run(config, db, gh, &args, false, None).await?;
            Ok(format!(
                "Re-analyzed PR #{number}. {}",
                if apply {
                    "Labels and comments updated."
                } else {
                    "(dry-run)"
                }
            ))
        }
        SlashCommand::Review => {
            if !is_pr {
                return Ok("This command only works on pull requests.".into());
            }
            info!("Slash command: review PR #{number}");
            gh_sync::incremental_sync(gh, db, "pulls").await?;
            let args = ReviewArgs {
                pr: Some(number),
                apply,
            };
            pipelines::review::run(config, db, gh, &args, false).await?;
            Ok(format!(
                "Reviewed PR #{number}. {}",
                if apply {
                    "Review comments posted."
                } else {
                    "(dry-run)"
                }
            ))
        }
        SlashCommand::Label(label) => {
            info!("Slash command: label #{number} with '{label}'");
            if apply {
                let labels = vec![label.clone()];
                if is_pr {
                    gh.label_pr(number, &labels).await?;
                } else {
                    gh.label_issue(number, &labels).await?;
                }
                Ok(format!("Added label `{label}` to #{number}."))
            } else {
                Ok(format!("Would add label `{label}` to #{number}. (dry-run)"))
            }
        }
        SlashCommand::Unlabel(label) => {
            info!("Slash command: unlabel #{number} '{label}'");
            if apply {
                gh.remove_label(number, label).await?;
                Ok(format!("Removed label `{label}` from #{number}."))
            } else {
                Ok(format!(
                    "Would remove label `{label}` from #{number}. (dry-run)"
                ))
            }
        }
        SlashCommand::Fix => {
            if is_pr {
                return Ok("This command only works on issues.".into());
            }
            info!("Slash command: fix issue #{number}");
            if !apply {
                return Ok(format!(
                    "Would auto-fix issue #{number}. (dry-run — start daemon with `--apply` to enable)"
                ));
            }

            // Check if the commenter is authorized
            if !config.fix.allowed_users.is_empty() {
                let user = triggered_by.unwrap_or("unknown");
                if !config.fix.allowed_users.iter().any(|u| u == user) {
                    return Ok(format!(
                        "User `{user}` is not authorized to trigger auto-fix. Allowed: {}",
                        config.fix.allowed_users.join(", ")
                    ));
                }
            }

            gh_sync::sync_issues_now(gh, db).await?;
            let fix_args = FixArgs {
                issue: number,
                tool: None,
                model: None,
                docker: false,
                image: None,
                apply: true,
            };
            match pipelines::autogen::run(config, db, gh, &fix_args, None).await {
                Ok(()) => Ok(format!(
                    "Auto-fix attempted for issue #{number}. Check for a new draft PR."
                )),
                Err(e) => Ok(format!("Auto-fix failed for issue #{number}: {e:#}")),
            }
        }
        SlashCommand::Queue => {
            if !is_pr {
                return Ok("This command only works on pull requests.".into());
            }
            info!("Slash command: queue position for PR #{number}");
            gh_sync::incremental_sync(gh, db, "pulls").await?;
            // Get all open PRs and compute scores
            let pulls = db.get_open_pulls()?;
            let mut scored: Vec<(u64, String, i32)> = pulls
                .iter()
                .map(|pr| {
                    let (score, _) = pipelines::pr_health::score_pr(pr);
                    (pr.number, pr.title.clone(), score)
                })
                .collect();
            scored.sort_by(|a, b| b.2.cmp(&a.2));

            let position = scored.iter().position(|(n, _, _)| *n == number);
            match position {
                Some(pos) => {
                    let (_, _, score) = &scored[pos];
                    Ok(format!(
                        "PR #{number} is **#{pos}** in the merge queue (score: {score}).\n\n\
                         Top 5:\n{}",
                        scored
                            .iter()
                            .take(5)
                            .enumerate()
                            .map(|(i, (n, title, s))| format!(
                                "{}. #{n} ({s} pts) — {title}",
                                i + 1
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ))
                }
                None => Ok(format!("PR #{number} is not in the merge queue.")),
            }
        }
        SlashCommand::Health => {
            if !is_pr {
                return Ok("This command only works on pull requests.".into());
            }
            info!("Slash command: health check PR #{number}");
            gh_sync::incremental_sync(gh, db, "pulls").await?;
            let pulls = db.get_open_pulls()?;
            let pr = pulls.iter().find(|p| p.number == number);
            match pr {
                Some(pr) => {
                    let age_days = {
                        let created = chrono::DateTime::parse_from_rfc3339(&pr.created_at).ok();
                        created
                            .map(|c| {
                                (chrono::Utc::now() - c.with_timezone(&chrono::Utc)).num_days()
                            })
                            .unwrap_or(0)
                    };
                    let (score, _) = pipelines::pr_health::score_pr(pr);
                    let stale = age_days > 14;
                    Ok(format!(
                        "**PR #{number} Health**\n\n\
                         | Metric | Value |\n|--------|-------|\n\
                         | Age | {age_days} days |\n\
                         | Score | {score} |\n\
                         | Labels | {} |\n\
                         | Mergeable | {} |\n\
                         | Stale | {} |",
                        if pr.labels.is_empty() {
                            "none".to_string()
                        } else {
                            pr.labels.join(", ")
                        },
                        pr.mergeable
                            .map(|m| if m { "yes" } else { "no" })
                            .unwrap_or("unknown"),
                        if stale { "yes" } else { "no" },
                    ))
                }
                None => Ok(format!("PR #{number} not found in cache.")),
            }
        }
        SlashCommand::Help => {
            let p = &config.branding.command_prefix;
            let name = &config.branding.name;
            Ok(format!(
                "**{name} bot commands**\n\n\
                 | Command | Description |\n|---------|-------------|\n\
                 | `{p} triage` | (Re)triage this issue |\n\
                 | `{p} analyze` | (Re)analyze this PR |\n\
                 | `{p} review` | Inline AI code review |\n\
                 | `{p} label <name>` | Add a label |\n\
                 | `{p} unlabel <name>` | Remove a label |\n\
                 | `{p} fix` | Auto-fix this issue |\n\
                 | `{p} queue` | Show merge queue position |\n\
                 | `{p} health` | PR health check |\n\
                 | `{p} help` | Show this help |"
            ))
        }
        SlashCommand::Unknown(msg) => Ok(format!(
            "Unknown command: `{msg}`.\n\nUse `{} help` to see available commands.",
            config.branding.command_prefix,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slash_prefix() {
        assert!(matches!(
            parse("/wshm fix", "/wshm"),
            Some(SlashCommand::Fix)
        ));
        assert!(matches!(
            parse("/wshm autofix", "/wshm"),
            Some(SlashCommand::Fix)
        ));
        assert!(matches!(
            parse("/wshm triage", "/wshm"),
            Some(SlashCommand::Triage)
        ));
    }

    #[test]
    fn test_at_prefix() {
        assert!(matches!(
            parse("@wshm fix", "/wshm"),
            Some(SlashCommand::Fix)
        ));
        assert!(matches!(
            parse("@wshm autofix", "/wshm"),
            Some(SlashCommand::Fix)
        ));
        assert!(matches!(
            parse("@wshm auto-fix", "/wshm"),
            Some(SlashCommand::Fix)
        ));
        assert!(matches!(
            parse("@wshm triage", "/wshm"),
            Some(SlashCommand::Triage)
        ));
    }

    #[test]
    fn test_at_prefix_with_extra_text() {
        // "please" after command is ignored
        assert!(matches!(
            parse("@wshm fix please", "/wshm"),
            Some(SlashCommand::Fix)
        ));
    }

    #[test]
    fn test_in_multiline_comment() {
        let comment = "Hey team,\nCan someone look at this?\n\n@wshm fix\n\nThanks!";
        assert!(matches!(parse(comment, "/wshm"), Some(SlashCommand::Fix)));
    }

    #[test]
    fn test_no_command() {
        assert!(parse("just a regular comment", "/wshm").is_none());
        assert!(parse("@grok do something", "/wshm").is_none());
    }
}
