use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::ai::backend::AiBackend;
use crate::ai::prompts::issue_classify;
use crate::ai::schemas::IssueClassification;
use crate::cli::{FixArgs, TriageArgs};
use crate::config::Config;
use crate::db::issues::Issue;
use crate::db::Database;
use crate::export::{EventKind, ExportEvent, ExportManager};
use crate::github::Client as GhClient;
use crate::pipelines::autogen;

#[derive(Serialize)]
struct TriageOutput {
    issue_number: u64,
    title: String,
    applied: bool,
    classification: IssueClassification,
}

/// Output format for triage results.
#[derive(Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Text,
    Json,
    Csv,
}

pub async fn run(
    config: &Config,
    db: &Database,
    gh: &GhClient,
    args: &TriageArgs,
    format: OutputFormat,
    exporter: Option<&ExportManager>,
) -> Result<()> {
    let json = format == OutputFormat::Json;
    let model = config.model_for("triage");
    let backend = AiBackend::from_config(config, model)?;

    let is_retriage = args.retriage;

    let issues = if let Some(number) = args.issue {
        match db.get_issue(number)? {
            Some(issue) => vec![issue],
            None => {
                if json {
                    println!("[]");
                } else {
                    println!("Issue #{number} not found in cache. Run `wshm sync` first.");
                }
                return Ok(());
            }
        }
    } else if is_retriage {
        // Re-triage: fetch issues whose triage result is stale
        let max_age = if config.triage.retriage_interval_hours > 0 {
            config.triage.retriage_interval_hours
        } else {
            24 // fallback when called manually with --retriage but no interval configured
        };
        let stale = db.get_stale_triage_results(max_age)?;
        if stale.is_empty() {
            if json {
                println!("[]");
            } else {
                println!("No stale triage results to re-evaluate.");
            }
            return Ok(());
        }
        // Fetch the full Issue objects for stale results
        let mut issues = Vec::with_capacity(stale.len());
        for row in &stale {
            if let Some(issue) = db.get_issue(row.issue_number)? {
                issues.push(issue);
            }
        }
        issues
    } else {
        db.get_untriaged_issues()?
    };

    if issues.is_empty() {
        if json {
            println!("[]");
        } else if is_retriage {
            println!("No issues to re-triage.");
        } else {
            println!("No issues to triage.");
        }
        return Ok(());
    }

    if is_retriage {
        info!("Re-triaging {} previously triaged issues", issues.len());
    }

    let existing_issues = db.get_open_issues()?;
    let open_prs = db.get_open_pulls()?;
    let mut results: Vec<TriageOutput> = Vec::with_capacity(issues.len());

    for issue in &issues {
        // Skip blacklisted issues
        if config.issues_blacklist.contains(&issue.number) {
            info!("Skipping blacklisted issue #{}", issue.number);
            continue;
        }

        info!("Triaging issue #{}: {}", issue.number, issue.title);
        match triage_issue(
            config,
            &backend,
            db,
            gh,
            issue,
            &existing_issues,
            &open_prs,
            args.apply,
            exporter,
        )
        .await
        {
            Ok(classification) => {
                if !json {
                    print_classification(issue, &classification, args.apply);
                }
                results.push(TriageOutput {
                    issue_number: issue.number,
                    title: issue.title.clone(),
                    applied: args.apply,
                    classification,
                });
            }
            Err(e) => {
                tracing::error!("Failed to triage issue #{}: {e:#}", issue.number);
            }
        }
    }

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&results)?);
        }
        OutputFormat::Csv => {
            println!("issue,title,category,confidence,priority,labels,simple_fix,relevant_files,summary");
            for r in &results {
                let c = &r.classification;
                println!(
                    "{},\"{}\",{},{:.0}%,{},\"{}\",{},\"{}\",\"{}\"",
                    r.issue_number,
                    r.title.replace('"', "\"\""),
                    c.category,
                    c.confidence * 100.0,
                    c.priority.as_deref().unwrap_or("unset"),
                    c.suggested_labels.join(";"),
                    c.is_simple_fix,
                    c.relevant_files.join(";"),
                    c.summary.replace('"', "\"\""),
                );
            }
        }
        OutputFormat::Text => {}
    }

    Ok(())
}

async fn triage_issue(
    config: &Config,
    ai: &AiBackend,
    db: &Database,
    gh: &GhClient,
    issue: &Issue,
    existing_issues: &[Issue],
    open_prs: &[crate::db::pulls::PullRequest],
    apply: bool,
    exporter: Option<&ExportManager>,
) -> Result<IssueClassification> {
    // ICM: recall past triage decisions and feedback for context
    let icm_context = crate::icm::recall_context(
        &format!(
            "triage issue classification {} {}",
            issue.title,
            config.repo_slug()
        ),
        5,
    );

    let mut user_prompt = issue_classify::build_user_prompt(issue, existing_issues, open_prs);

    // Inject custom label definitions if configured
    let labels_prompt = config.labels_prompt();
    if !labels_prompt.is_empty() {
        user_prompt.push_str(&labels_prompt);
    }

    if !icm_context.is_empty() {
        user_prompt.push_str(&format!(
            "\n\n## Past triage context (from memory)\n{icm_context}"
        ));
    }

    let classification: IssueClassification =
        ai.complete(issue_classify::SYSTEM, &user_prompt).await?;

    // Only persist triage result and ICM context when applying
    if apply {
        db.upsert_triage_result(&classification, issue.number)?;
    }

    // ICM: store triage decision for future context (only when applying)
    if apply {
        crate::icm::store(
            &format!("triage-{}", config.repo_slug()),
            &format!(
                "Issue #{} '{}' → {} (confidence: {:.0}%, priority: {})",
                issue.number,
                issue.title,
                classification.category,
                classification.confidence * 100.0,
                classification.priority.as_deref().unwrap_or("unset"),
            ),
            "low",
            &["triage", &classification.category],
        );
    }

    if apply && classification.confidence >= config.triage.auto_fix_confidence {
        // Build new label set
        let mut new_labels = classification.suggested_labels.clone();
        if let Some(ref priority) = classification.priority {
            let priority_label = format!("priority:{priority}");
            if !new_labels.contains(&priority_label) {
                new_labels.push(priority_label);
            }
        }
        let new_labels = config.filter_labels(new_labels);

        // Get labels previously applied by wshm (to know what to remove on re-triage)
        let old_wshm_labels = db.get_wshm_applied_labels(issue.number)?;

        // Labels to remove: previously applied by wshm but not in the new set
        let to_remove: Vec<String> = old_wshm_labels
            .iter()
            .filter(|old| !new_labels.iter().any(|new| new.eq_ignore_ascii_case(old)))
            .cloned()
            .collect();

        // Remove stale wshm labels from GitHub
        for label in &to_remove {
            if let Err(e) = gh.remove_label(issue.number, label).await {
                tracing::warn!("Failed to remove label '{label}' from #{}: {e}", issue.number);
            }
        }

        // Add new labels on GitHub (additive, no-op if already present)
        if !new_labels.is_empty() {
            gh.label_issue(issue.number, &new_labels).await?;
        }

        // Update DB cache: merge (remove old wshm labels, add new ones, keep human labels)
        db.merge_issue_labels(issue.number, &new_labels, &to_remove)?;

        // Auto-assign issue
        if config.assign.enabled {
            if let Some(assignee) = crate::config::AssignConfig::pick(&config.assign.issues) {
                info!("Auto-assigning issue #{} to {assignee}", issue.number);
                gh.add_assignees(issue.number, &[assignee.to_string()]).await?;
            }
        }

        // Post triage comment
        let comment = format_triage_comment(&classification, config);
        gh.comment_issue(issue.number, &comment).await?;

        // Handle special categories
        match classification.category.as_str() {
            "duplicate" => {
                if let Some(original) = classification.is_duplicate_of {
                    let close_msg = format!(
                        "Closing as duplicate of #{original}. See the original issue for updates."
                    );
                    gh.comment_issue(issue.number, &close_msg).await?;
                    gh.close_issue(issue.number).await?;
                }
            }
            "wontfix" => {
                gh.close_issue(issue.number).await?;
            }
            _ => {}
        }

        // Auto-fix: if enabled and issue is a simple fix with high confidence
        if config.triage.auto_fix
            && classification.is_simple_fix
            && matches!(classification.category.as_str(), "bug" | "feature")
            && classification.confidence >= config.triage.auto_fix_confidence
        {
            info!(
                "Auto-fix triggered for issue #{} (confidence: {:.0}%)",
                issue.number,
                classification.confidence * 100.0
            );
            let fix_args = FixArgs {
                issue: issue.number,
                tool: None,
                model: None,
                docker: false,
                image: None,
                apply: true,
            };
            match autogen::run(config, db, gh, &fix_args, exporter).await {
                Ok(()) => info!("Auto-fix completed for issue #{}", issue.number),
                Err(e) => tracing::error!("Auto-fix failed for issue #{}: {e:#}", issue.number),
            }
        }

        info!("Applied triage to issue #{}", issue.number);

        // Emit export event
        if let Some(em) = exporter {
            em.emit(&ExportEvent {
                kind: EventKind::IssueTriaged,
                repo: config.repo_slug(),
                timestamp: chrono::Utc::now(),
                data: serde_json::to_value(&classification)?,
            })
            .await?;
        }
    }

    Ok(classification)
}

fn will_auto_fix(c: &IssueClassification, config: &Config) -> bool {
    config.triage.auto_fix
        && c.is_simple_fix
        && matches!(c.category.as_str(), "bug" | "feature")
        && c.confidence >= config.triage.auto_fix_confidence
}

fn category_emoji(cat: &str) -> &'static str {
    match cat {
        "bug" => "🐛",
        "feature" => "✨",
        "duplicate" => "♻️",
        "wontfix" => "🚫",
        "needs-info" => "❓",
        _ => "📋",
    }
}

fn priority_emoji(pri: Option<&str>) -> &'static str {
    match pri {
        Some("critical") => "🔴",
        Some("high") => "🟠",
        Some("medium") => "🟡",
        Some("low") => "🟢",
        _ => "⚪",
    }
}

fn format_triage_comment(c: &IssueClassification, config: &Config) -> String {
    let cat_emoji = category_emoji(&c.category);
    let pri_emoji = priority_emoji(c.priority.as_deref());
    let priority = c.priority.as_deref().unwrap_or("unset");
    let confidence = format!("{:.0}", c.confidence * 100.0);
    let header = config.branding.header();
    let footer = config.branding.footer("Triaged");

    let relevant_files = if c.relevant_files.is_empty() {
        String::new()
    } else {
        let files: Vec<String> = c.relevant_files.iter().map(|f| format!("- `{f}`")).collect();
        format!(
            "<details>\n<summary>📁 Relevant files</summary>\n\n{}\n\n</details>",
            files.join("\n")
        )
    };

    let duplicate_of = c
        .is_duplicate_of
        .as_ref()
        .map(|d| format!("> ♻️ Possible duplicate of #{d}"))
        .unwrap_or_default();

    // Use custom template if provided
    if let Some(ref tmpl) = config.branding.triage_template {
        return tmpl
            .replace("{category}", &c.category)
            .replace("{priority}", priority)
            .replace("{confidence}", &confidence)
            .replace("{summary}", &c.summary)
            .replace("{category_emoji}", cat_emoji)
            .replace("{priority_emoji}", pri_emoji)
            .replace("{relevant_files}", &relevant_files)
            .replace("{duplicate_of}", &duplicate_of)
            .replace("{header}", &header)
            .replace("{footer}", &footer);
    }

    // Default template
    let mut comment = header;

    comment.push_str(&format!(
        "## 🔍 Automated Triage\n\n\
         | | |\n|---|---|\n\
         | {cat_emoji} **Category** | `{}` |\n\
         | {pri_emoji} **Priority** | `{priority}` |\n\
         | 🎯 **Confidence** | {confidence}% |\n\n\
         ### Summary\n\n\
         {}\n",
        c.category, c.summary,
    ));

    if c.is_simple_fix {
        if will_auto_fix(c, config) {
            comment.push_str("\n> 🔧 This looks like a **trivial fix** — attempting auto-fix now. A draft PR will be opened for review.\n");
        } else {
            comment.push_str(&format!(
                "\n> 💡 This looks like a **simple fix** that could be auto-resolved. Use `{} fix` to attempt it.\n",
                config.branding.command_prefix
            ));
        }
    }

    if !duplicate_of.is_empty() {
        comment.push_str(&format!("\n{duplicate_of}\n"));
    }

    if !relevant_files.is_empty() {
        comment.push_str(&format!("\n{relevant_files}\n"));
    }

    comment.push_str(&format!("\n{footer}"));
    comment
}

fn print_classification(issue: &Issue, c: &IssueClassification, applied: bool) {
    let status = if applied {
        "\x1b[32mAPPLIED\x1b[0m"
    } else {
        "\x1b[33mDRY-RUN\x1b[0m"
    };

    let cat_color = match c.category.as_str() {
        "bug" => "\x1b[31m",        // red
        "feature" => "\x1b[36m",    // cyan
        "duplicate" => "\x1b[90m",  // gray
        "wontfix" => "\x1b[90m",    // gray
        "needs-info" => "\x1b[33m", // yellow
        _ => "\x1b[37m",            // white
    };

    let pri_color = match c.priority.as_deref() {
        Some("critical") => "\x1b[31;1m",
        Some("high") => "\x1b[33;1m",
        Some("medium") => "\x1b[33m",
        Some("low") => "\x1b[32m",
        _ => "\x1b[37m",
    };

    let labels = if c.suggested_labels.is_empty() {
        String::new()
    } else {
        let colored: Vec<String> = c.suggested_labels.iter()
            .map(|l| format!("\x1b[35m{l}\x1b[0m"))
            .collect();
        format!(" [{}]", colored.join(", "))
    };

    println!(
        "[{status}] #{} {} → {cat_color}{}\x1b[0m ({:.0}%, {pri_color}{}\x1b[0m){labels}",
        issue.number,
        crate::pipelines::truncate(&issue.title, 60),
        c.category,
        c.confidence * 100.0,
        c.priority.as_deref().unwrap_or("unset"),
    );
}
