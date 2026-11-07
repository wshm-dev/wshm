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

use crate::config::Config;
use crate::git_provider::Forge as GhClient;
use crate::github::sync as gh_sync;
use crate::pipelines;
use crate::pro_hooks;

/// A parsed slash command from a comment body.
#[derive(Debug)]
pub enum SlashCommand {
    Triage,
    Analyze,
    Review,
    Label(String),
    Unlabel(String),
    /// `fix [--force]`: `--force` overrides the injection gate (#235).
    Fix {
        force: bool,
    },
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
                Some(ref c) if c == "fix" || c == "autofix" || c == "auto-fix" => {
                    SlashCommand::Fix {
                        force: parts.iter().skip(1).any(|p| *p == "--force"),
                    }
                }
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

/// Check if a user is authorized to run slash commands (collaborator or in allowed_users).
async fn authorize_command(
    gh: &GhClient,
    config: &Config,
    triggered_by: Option<&str>,
    cmd_name: &str,
) -> Result<Option<String>> {
    let user = triggered_by.unwrap_or("unknown");

    // allowed_users whitelist (if configured)
    if !config.fix.allowed_users.is_empty() {
        if config.fix.allowed_users.iter().any(|u| u == user) {
            return Ok(None); // authorized
        }
        return Ok(Some(format!(
            "User `{user}` is not authorized to run `{cmd_name}`. Allowed: {}",
            config.fix.allowed_users.join(", ")
        )));
    }

    // Default: collaborator check
    match gh.is_collaborator(user).await {
        Ok(true) => Ok(None), // authorized
        Ok(false) => Ok(Some(format!(
            "User `{user}` is not a repo collaborator. Only collaborators can run `{cmd_name}`."
        ))),
        Err(e) => {
            tracing::warn!("Failed to check collaborator status for {user}: {e}");
            Ok(Some(format!(
                "Could not verify authorization for `{user}`. Please try again later."
            )))
        }
    }
}

/// Execute a slash command and return a response comment body.
///
/// `triage`, `analyze` and `review` are partial runs of the item's pipeline
/// (only the steps of that kind, trigger `command`, recorded): the template
/// and the step conditions apply, except the repo toggles (`features`,
/// `repo_filters`), which govern automatic processing and never blocked an
/// explicit command. The other commands do not belong to a pipeline.
pub async fn execute(
    cmd: &SlashCommand,
    is_pr: bool,
    ctx: &pipelines::engine::RunContext,
    triggered_by: Option<&str>,
) -> Result<String> {
    let config: &Config = &ctx.config;
    let db = ctx.db.as_ref();
    let gh: &GhClient = ctx.gh.as_ref();
    let apply = ctx.apply;
    let Some(number) = ctx.item.number else {
        return Ok(String::new());
    };

    // Authorize all commands except Help and Unknown
    if !matches!(cmd, SlashCommand::Help | SlashCommand::Unknown(_)) {
        let cmd_name = format!("{:?}", cmd);
        if let Some(deny_msg) = authorize_command(gh, config, triggered_by, &cmd_name).await? {
            return Ok(deny_msg);
        }
    }

    match cmd {
        SlashCommand::Triage => {
            info!("Slash command: triage issue #{number}");
            gh_sync::incremental_sync(gh, db, "issues").await?;
            let report = run_command_steps(ctx, "triage").await?;
            Ok(match command_outcome(&report, "triage", ctx) {
                Ok(()) => format!(
                    "Re-triaged issue #{number}. {}",
                    if apply {
                        "Labels and comments updated."
                    } else {
                        "(dry-run — use `--apply` on daemon to enable actions)"
                    }
                ),
                Err(msg) => msg,
            })
        }
        SlashCommand::Analyze => {
            if !is_pr {
                return Ok("This command only works on pull requests.".into());
            }
            info!("Slash command: analyze PR #{number}");
            gh_sync::incremental_sync(gh, db, "pulls").await?;
            // The analyze step of a command run generates no diagram in the
            // command's task (a cached one is kept; the next push
            // regenerates it in the background).
            let report = run_command_steps(ctx, "analyze").await?;
            Ok(match command_outcome(&report, "analyze", ctx) {
                Ok(()) => format!(
                    "Re-analyzed PR #{number}. {}",
                    if apply {
                        "Labels and comments updated."
                    } else {
                        "(dry-run)"
                    }
                ),
                Err(msg) => msg,
            })
        }
        SlashCommand::Review => {
            if !is_pr {
                return Ok("This command only works on pull requests.".into());
            }
            info!("Slash command: review PR #{number}");
            gh_sync::incremental_sync(gh, db, "pulls").await?;
            // The template's whole review chain: with a collect-mode review,
            // `verify` is what posts.
            let report = run_command_kinds(ctx, pipelines::engine::REVIEW_KINDS).await?;
            Ok(match command_outcome(&report, "review", ctx) {
                Ok(()) => review_reply(&report, number, apply),
                Err(msg) => msg,
            })
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
                if is_pr {
                    gh.remove_pr_label(number, label).await?;
                } else {
                    gh.remove_label(number, label).await?;
                }
                Ok(format!("Removed label `{label}` from #{number}."))
            } else {
                Ok(format!(
                    "Would remove label `{label}` from #{number}. (dry-run)"
                ))
            }
        }
        SlashCommand::Fix { force } => {
            if is_pr {
                return Ok("This command only works on issues.".into());
            }
            info!("Slash command: fix issue #{number}");
            if !apply {
                return Ok(format!(
                    "Would auto-fix issue #{number}. (dry-run — start daemon with `--apply` to enable)"
                ));
            }

            // Auth already checked globally above. The injection gate (#235)
            // runs in autogen::run: a coding agent with write access would run
            // on the issue text. A maintainer can override it with --force.
            gh_sync::sync_issues_now(gh, db).await?;
            match pro_hooks::run_auto_fix(config, db, gh, number, *force).await {
                Ok(_) => Ok(format!(
                    "Auto-fix attempted for issue #{number}. Check for a new draft PR."
                )),
                Err(e) if e.is::<pipelines::autogen::AutoFixRefused>() => Ok(format!(
                    "Auto-fix refused for issue #{number}: {e}. Review the issue, then use `{} fix --force` to run it anyway.",
                    config.branding.command_prefix
                )),
                Err(e) => {
                    tracing::warn!("auto-fix #{number} failed: {e:#}");
                    Ok(format!(
                        "Auto-fix failed for issue #{number}. See server logs for details."
                    ))
                }
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
            scored.sort_by_key(|b| std::cmp::Reverse(b.2));

            let position = scored.iter().position(|(n, _, _)| *n == number);
            match position {
                Some(pos) => {
                    let (_, _, score) = &scored[pos];
                    Ok(format!(
                        "PR #{number} is **#{pos}** in the merge queue (score: {score}).

\
                         Top 5:
{}",
                        scored
                            .iter()
                            .take(5)
                            .enumerate()
                            .map(|(i, (n, title, s))| format!(
                                "{}. #{n} ({s} pts) — {title}",
                                i + 1
                            ))
                            .collect::<Vec<_>>()
                            .join(
                                "
"
                            )
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
                        "**PR #{number} Health**

\
                         | Metric | Value |
|--------|-------|
\
                         | Age | {age_days} days |
\
                         | Score | {score} |
\
                         | Labels | {} |
\
                         | Mergeable | {} |
\
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
                "**{name} bot commands**

\
                 | Command | Description |
|---------|-------------|
\
                 | `{p} triage` | (Re)triage this issue |
\
                 | `{p} analyze` | (Re)analyze this PR |
\
                 | `{p} review` | Inline AI code review |
\
                 | `{p} label <name>` | Add a label |
\
                 | `{p} unlabel <name>` | Remove a label |
\
                 | `{p} fix` | Auto-fix this issue |
\
                 | `{p} queue` | Show merge queue position |
\
                 | `{p} health` | PR health check |
\
                 | `{p} help` | Show this help |"
            ))
        }
        SlashCommand::Unknown(msg) => Ok(format!(
            "Unknown command: `{msg}`.

Use `{} help` to see available commands.",
            config.branding.command_prefix,
        )),
    }
}

/// Run the steps of `kind` of the item's pipeline for a slash command.
/// `Err` when a step with `on_failure = "abort"` failed (the command
/// fails, as it did when it called the pipeline directly).
async fn run_command_steps(
    ctx: &pipelines::engine::RunContext,
    kind: &str,
) -> Result<pipelines::engine::RunReport> {
    run_command_kinds(ctx, &[kind]).await
}

/// What `/wshm review` really did, from the summaries of its steps.
fn review_reply(report: &pipelines::engine::RunReport, number: u64, apply: bool) -> String {
    use pipelines::engine::StepStatus;
    let line = |uses: &str| {
        report
            .steps
            .iter()
            .find(|s| s.uses == uses)
            .map(|s| match s.status {
                StepStatus::Done => {
                    format!("{}: {}", s.step_id, s.summary.as_deref().unwrap_or("done"))
                }
                _ => format!(
                    "{}: {} ({})",
                    s.step_id,
                    s.status.as_str(),
                    s.reason.as_deref().unwrap_or("")
                ),
            })
    };
    let mut parts: Vec<String> = ["review", "security", "verify", "status_check"]
        .iter()
        .filter_map(|u| line(u))
        .collect();
    if !apply {
        parts.push("dry-run: nothing posted".into());
    }
    format!("Reviewed PR #{number}.\n\n- {}", parts.join("\n- "))
}

async fn run_command_kinds(
    ctx: &pipelines::engine::RunContext,
    kinds: &[&str],
) -> Result<pipelines::engine::RunReport> {
    let mut report = pipelines::engine::run_kinds(
        ctx,
        kinds,
        pipelines::engine::ExecOptions {
            ignore_toggles: true,
            ..Default::default()
        },
    )
    .await;
    if let Some((id, e)) = report.failure.take() {
        return Err(e.context(format!("pipeline step '{id}' failed")));
    }
    Ok(report)
}

/// `Ok` when a step of `kind` ran; else the reply explaining why not.
fn command_outcome(
    report: &pipelines::engine::RunReport,
    kind: &str,
    ctx: &pipelines::engine::RunContext,
) -> std::result::Result<(), String> {
    use pipelines::engine::StepStatus;
    let steps: Vec<_> = report.steps.iter().filter(|s| s.uses == kind).collect();
    if steps.iter().any(|s| s.status == StepStatus::Done) {
        return Ok(());
    }
    match steps.last() {
        None => Err(format!(
            "Pipeline `{}` has no `{kind}` step for {}, nothing done.",
            report.template, ctx.item
        )),
        Some(s) if s.status == StepStatus::Failed => Err(format!(
            "`{kind}` failed on {}: {}",
            ctx.item,
            s.reason.as_deref().unwrap_or("see the server logs")
        )),
        Some(s) => Err(format!(
            "`{kind}` skipped on {} by pipeline `{}`: {}",
            ctx.item,
            report.template,
            s.reason.as_deref().unwrap_or("")
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
            Some(SlashCommand::Fix { .. })
        ));
        assert!(matches!(
            parse("/wshm autofix", "/wshm"),
            Some(SlashCommand::Fix { .. })
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
            Some(SlashCommand::Fix { .. })
        ));
        assert!(matches!(
            parse("@wshm autofix", "/wshm"),
            Some(SlashCommand::Fix { .. })
        ));
        assert!(matches!(
            parse("@wshm auto-fix", "/wshm"),
            Some(SlashCommand::Fix { .. })
        ));
        assert!(matches!(
            parse("@wshm triage", "/wshm"),
            Some(SlashCommand::Triage)
        ));
    }

    #[test]
    fn test_at_prefix_with_extra_text() {
        assert!(matches!(
            parse("@wshm fix please", "/wshm"),
            Some(SlashCommand::Fix { .. })
        ));
    }

    #[test]
    fn test_in_multiline_comment() {
        let comment = "Hey team,
Can someone look at this?

@wshm fix

Thanks!";
        assert!(matches!(
            parse(comment, "/wshm"),
            Some(SlashCommand::Fix { .. })
        ));
    }

    #[test]
    fn test_no_command() {
        assert!(parse("just a regular comment", "/wshm").is_none());
        assert!(parse("@grok do something", "/wshm").is_none());
    }
}
