use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::ai::backend::AiBackend;
use crate::ai::prompts::pr_analyze;
use crate::ai::schemas::PrAnalysis;
use crate::cli::PrArgs;
use crate::config::Config;
use crate::db::pulls::PullRequest;
use crate::db::Database;
use crate::export::{EventKind, ExportEvent, ExportManager};
use crate::github::Client as GhClient;

#[derive(Serialize)]
struct PrAnalysisOutput {
    pr_number: u64,
    title: String,
    applied: bool,
    analysis: PrAnalysis,
}

pub async fn run(
    config: &Config,
    db: &Database,
    gh: &GhClient,
    args: &PrArgs,
    json: bool,
    exporter: Option<&ExportManager>,
) -> Result<()> {
    let model = config.model_for("pr");
    let ai = AiBackend::from_config(config, model)?;

    let pulls = if let Some(number) = args.pr {
        match db.get_pull(number)? {
            Some(pr) => vec![pr],
            None => {
                if json {
                    println!("[]");
                } else {
                    println!("PR #{number} not found in cache. Run `wshm sync` first.");
                }
                return Ok(());
            }
        }
    } else {
        db.get_unanalyzed_pulls()?
    };

    if pulls.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No PRs to analyze.");
        }
        return Ok(());
    }

    let mut results: Vec<PrAnalysisOutput> = Vec::with_capacity(pulls.len());

    for pr in &pulls {
        if config.prs_blacklist.contains(&pr.number) {
            info!("Skipping blacklisted PR #{}", pr.number);
            continue;
        }

        info!("Analyzing PR #{}: {}", pr.number, pr.title);
        match analyze_pr(config, &ai, db, gh, pr, args.apply, exporter).await {
            Ok(analysis) => {
                if !json {
                    print_analysis(pr, &analysis, args.apply);
                }
                results.push(PrAnalysisOutput {
                    pr_number: pr.number,
                    title: pr.title.clone(),
                    applied: args.apply,
                    analysis,
                });
            }
            Err(e) => {
                tracing::error!("Failed to analyze PR #{}: {e:#}", pr.number);
            }
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }

    Ok(())
}

async fn analyze_pr(
    config: &Config,
    ai: &AiBackend,
    db: &Database,
    gh: &GhClient,
    pr: &PullRequest,
    apply: bool,
    exporter: Option<&ExportManager>,
) -> Result<PrAnalysis> {
    // Try to fetch diff (best-effort)
    let diff = match gh.fetch_pr_diff(pr.number).await {
        Ok(d) => Some(d),
        Err(e) => {
            tracing::warn!("Could not fetch diff for PR #{}: {e}", pr.number);
            None
        }
    };

    // ICM: recall past PR analysis context
    let icm_context = crate::icm::recall_context(
        &format!("pr analysis {} {}", pr.title, config.repo_slug()),
        5,
    );

    let mut user_prompt = pr_analyze::build_user_prompt(pr, diff.as_deref());

    // Inject custom label definitions if configured
    let labels_prompt = config.labels_prompt();
    if !labels_prompt.is_empty() {
        user_prompt.push_str(&labels_prompt);
    }

    if !icm_context.is_empty() {
        user_prompt.push_str(&format!(
            "\n\n## Past PR analysis context (from memory)\n{icm_context}"
        ));
    }

    let system_prompt = config.pr.system_prompt.as_deref()
        .unwrap_or(pr_analyze::SYSTEM);

    let analysis: PrAnalysis = ai.complete(system_prompt, &user_prompt).await?;

    // Store in DB
    let now = chrono::Utc::now().to_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO pr_analyses (pr_number, summary, risk_level, pr_type, review_notes, analyzed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(pr_number) DO UPDATE SET
                summary = excluded.summary,
                risk_level = excluded.risk_level,
                pr_type = excluded.pr_type,
                review_notes = excluded.review_notes,
                analyzed_at = excluded.analyzed_at",
            rusqlite::params![
                pr.number,
                analysis.summary,
                analysis.risk_level,
                analysis.pr_type,
                serde_json::to_string(&analysis.review_checklist)?,
                now,
            ],
        )?;
        Ok(())
    })?;

    // ICM: store PR analysis for future context
    crate::icm::store(
        &format!("pr-analysis-{}", config.repo_slug()),
        &format!(
            "PR #{} '{}' → {} (risk: {}, type: {})",
            pr.number,
            crate::pipelines::truncate(&pr.title, 80),
            crate::pipelines::truncate(&analysis.summary, 120),
            analysis.risk_level,
            analysis.pr_type,
        ),
        "low",
        &["pr-analysis", &analysis.pr_type, &analysis.risk_level],
    );

    if apply {
        let labels = config.filter_labels(analysis.suggested_labels.clone());
        if !labels.is_empty() {
            gh.label_pr(pr.number, &labels).await?;
        }

        // Auto-assign PR
        if config.assign.enabled {
            if let Some(assignee) = crate::config::AssignConfig::pick(&config.assign.prs) {
                info!("Auto-assigning PR #{} to {assignee}", pr.number);
                gh.add_assignees(pr.number, &[assignee.to_string()]).await?;
            }
        }

        let comment = format_analysis_comment(&analysis, config);
        gh.comment_pr(pr.number, &comment).await?;

        info!("Applied analysis to PR #{}", pr.number);

        // Emit export event
        if let Some(em) = exporter {
            em.emit(&ExportEvent {
                kind: EventKind::PrAnalyzed,
                repo: config.repo_slug(),
                timestamp: chrono::Utc::now(),
                data: serde_json::to_value(&analysis)?,
            })
            .await?;
        }
    }

    Ok(analysis)
}

fn format_analysis_comment(a: &PrAnalysis, config: &Config) -> String {
    let risk_emoji = match a.risk_level.as_str() {
        "high" => "🔴",
        "medium" => "🟡",
        "low" => "🟢",
        _ => "⚪",
    };

    let type_emoji = match a.pr_type.as_str() {
        "bug-fix" => "🐛",
        "feature" => "✨",
        "refactor" => "♻️",
        "docs" => "📝",
        "chore" => "🔧",
        _ => "📋",
    };

    let check = |b: bool| if b { "x" } else { " " };
    let header = config.branding.header();
    let footer = config.branding.footer("Analyzed");
    let linked_issues = if a.linked_issues.is_empty() {
        String::new()
    } else {
        let links: Vec<String> = a.linked_issues.iter().map(|n| format!("#{n}")).collect();
        format!("**Linked issues:** {}", links.join(", "))
    };

    // Use custom template if provided
    if let Some(ref tmpl) = config.branding.pr_template {
        return tmpl
            .replace("{type}", &a.pr_type)
            .replace("{risk}", &a.risk_level)
            .replace("{summary}", &crate::pipelines::truncate(&a.summary, 500))
            .replace("{type_emoji}", type_emoji)
            .replace("{risk_emoji}", risk_emoji)
            .replace("{tests_present}", check(a.review_checklist.tests_present))
            .replace("{breaking_change}", check(a.review_checklist.breaking_change))
            .replace("{docs_updated}", check(a.review_checklist.docs_updated))
            .replace("{linked_issues}", &linked_issues)
            .replace("{header}", &header)
            .replace("{footer}", &footer);
    }

    // Default template
    let mut comment = header;

    comment.push_str(&format!(
        "## 📊 Automated PR Analysis\n\n\
         | | |\n|---|---|\n\
         | {type_emoji} **Type** | `{}` |\n\
         | {risk_emoji} **Risk** | `{}` |\n\n\
         ### Summary\n\n\
         {}\n\n\
         ### Review Checklist\n\
         - [{}] Tests present\n\
         - [{}] Breaking change\n\
         - [{}] Docs updated\n",
        a.pr_type,
        a.risk_level,
        crate::pipelines::truncate(&a.summary, 500),
        check(a.review_checklist.tests_present),
        check(a.review_checklist.breaking_change),
        check(a.review_checklist.docs_updated),
    ));

    if !linked_issues.is_empty() {
        comment.push_str(&format!("\n{linked_issues}\n"));
    }

    comment.push_str(&format!("\n{footer}"));
    comment
}

fn print_analysis(pr: &PullRequest, a: &PrAnalysis, applied: bool) {
    let status = if applied { "APPLIED" } else { "DRY-RUN" };
    println!(
        "[{status}] #{} {} → {} (risk: {}, type: {})",
        pr.number, pr.title, a.summary, a.risk_level, a.pr_type,
    );
}
