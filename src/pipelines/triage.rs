use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::ai::local::LocalClient;
use crate::ai::prompts::issue_classify;
use crate::ai::schemas::IssueClassification;
use crate::ai::AiClient;
use crate::cli::{FixArgs, TriageArgs};
use crate::config::Config;
use crate::db::issues::Issue;
use crate::db::Database;
use crate::github::Client as GhClient;
use crate::pipelines::autogen;

#[derive(Serialize)]
struct TriageOutput {
    issue_number: u64,
    title: String,
    applied: bool,
    classification: IssueClassification,
}

enum AiBackend {
    Remote(AiClient),
    Local(LocalClient),
}

impl AiBackend {
    async fn classify(&self, system: &str, user: &str) -> Result<IssueClassification> {
        match self {
            AiBackend::Remote(ai) => ai.complete(system, user).await,
            AiBackend::Local(local) => local.complete(system, user),
        }
    }
}

pub async fn run(
    config: &Config,
    db: &Database,
    gh: &GhClient,
    args: &TriageArgs,
    json: bool,
) -> Result<()> {
    let model = config.model_for("triage");
    let backend = if config.ai.provider == "local" {
        AiBackend::Local(LocalClient::new(model)?)
    } else {
        AiBackend::Remote(AiClient::with_model(config, model)?)
    };

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
    } else {
        db.get_untriaged_issues()?
    };

    if issues.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No issues to triage.");
        }
        return Ok(());
    }

    let existing_issues = db.get_open_issues()?;
    let mut results: Vec<TriageOutput> = Vec::new();

    for issue in &issues {
        info!("Triaging issue #{}: {}", issue.number, issue.title);
        match triage_issue(
            config,
            &backend,
            db,
            gh,
            issue,
            &existing_issues,
            args.apply,
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

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
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
    apply: bool,
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

    let mut user_prompt = issue_classify::build_user_prompt(issue, existing_issues);
    if !icm_context.is_empty() {
        user_prompt.push_str(&format!(
            "\n\n## Past triage context (from memory)\n{icm_context}"
        ));
    }

    let classification: IssueClassification =
        ai.classify(issue_classify::SYSTEM, &user_prompt).await?;

    // Store result in DB
    db.upsert_triage_result(&classification, issue.number)?;

    // ICM: store triage decision for future context
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

    if apply && classification.confidence >= config.triage.auto_fix_confidence {
        // Apply labels
        if !classification.suggested_labels.is_empty() {
            gh.label_issue(issue.number, &classification.suggested_labels)
                .await?;
            db.update_issue_labels(issue.number, &classification.suggested_labels)?;
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
            && classification.category == "bug"
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
            match autogen::run(config, db, gh, &fix_args).await {
                Ok(()) => info!("Auto-fix completed for issue #{}", issue.number),
                Err(e) => tracing::error!("Auto-fix failed for issue #{}: {e:#}", issue.number),
            }
        }

        info!("Applied triage to issue #{}", issue.number);
    }

    Ok(classification)
}

fn will_auto_fix(c: &IssueClassification, config: &Config) -> bool {
    config.triage.auto_fix
        && c.is_simple_fix
        && c.category == "bug"
        && c.confidence >= config.triage.auto_fix_confidence
}

fn format_triage_comment(c: &IssueClassification, config: &Config) -> String {
    let mut comment = config.branding.header();

    let priority_emoji = match c.priority.as_deref() {
        Some("critical") => "🔴",
        Some("high") => "🟠",
        Some("medium") => "🟡",
        Some("low") => "🟢",
        _ => "⚪",
    };

    let category_emoji = match c.category.as_str() {
        "bug" => "🐛",
        "feature" => "✨",
        "duplicate" => "♻️",
        "wontfix" => "🚫",
        "needs-info" => "❓",
        _ => "📋",
    };

    comment.push_str(&format!(
        "## 🔍 Automated Triage\n\n\
         | | |\n|---|---|\n\
         | {category_emoji} **Category** | `{}` |\n\
         | {priority_emoji} **Priority** | `{}` |\n\
         | 🎯 **Confidence** | {:.0}% |\n\n\
         ### Summary\n\n\
         {}\n",
        c.category,
        c.priority.as_deref().unwrap_or("unset"),
        c.confidence * 100.0,
        c.summary,
    ));

    if c.is_simple_fix {
        if will_auto_fix(c, config) {
            comment.push_str("\n> 🔧 This looks like a **trivial fix** — attempting auto-fix now. A draft PR will be opened for review.\n");
        } else {
            comment.push_str("\n> 💡 This looks like a **simple fix** that could be auto-resolved. Use `/wshm fix` to attempt it.\n");
        }
    }

    if let Some(ref dup) = c.is_duplicate_of {
        comment.push_str(&format!("\n> ♻️ Possible duplicate of #{dup}\n"));
    }

    if !c.relevant_files.is_empty() {
        comment.push_str("\n<details>\n<summary>📁 Relevant files</summary>\n\n");
        for f in &c.relevant_files {
            comment.push_str(&format!("- `{f}`\n"));
        }
        comment.push_str("\n</details>\n");
    }

    comment.push_str(&format!("\n{}", config.branding.footer("Triaged")));
    comment
}

fn print_classification(issue: &Issue, c: &IssueClassification, applied: bool) {
    let status = if applied { "APPLIED" } else { "DRY-RUN" };
    println!(
        "[{status}] #{} {} → {} (confidence: {:.0}%, priority: {})",
        issue.number,
        issue.title,
        c.category,
        c.confidence * 100.0,
        c.priority.as_deref().unwrap_or("unset"),
    );
}
