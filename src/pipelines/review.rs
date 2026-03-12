use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::ai::local::LocalClient;
use crate::ai::prompts::inline_review;
use crate::ai::schemas::InlineReviewResult;
use crate::ai::AiClient;
use crate::cli::ReviewArgs;
use crate::config::Config;
use crate::db::Database;
use crate::github::Client as GhClient;

#[derive(Serialize)]
struct ReviewOutput {
    pr_number: u64,
    title: String,
    applied: bool,
    additions: usize,
    deletions: usize,
    files_changed: usize,
    review: InlineReviewResult,
}

enum AiBackend {
    Remote(AiClient),
    Local(LocalClient),
}

impl AiBackend {
    async fn review(&self, system: &str, user: &str) -> Result<InlineReviewResult> {
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
    args: &ReviewArgs,
    json: bool,
) -> Result<()> {
    let ai = if config.ai.provider == "local" {
        AiBackend::Local(LocalClient::new(&config.ai.model)?)
    } else {
        AiBackend::Remote(AiClient::new(config)?)
    };

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
        db.get_open_pulls()?
    };

    if pulls.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No PRs to review.");
        }
        return Ok(());
    }

    let mut results: Vec<ReviewOutput> = Vec::new();

    for pr in &pulls {
        info!("Reviewing PR #{}: {}", pr.number, pr.title);

        // Fetch the raw diff
        let diff = match gh.fetch_pr_diff_raw(pr.number).await {
            Ok(d) if !d.is_empty() => d,
            Ok(_) => {
                if !json {
                    println!("  PR #{}: empty diff, skipping.", pr.number);
                }
                continue;
            }
            Err(e) => {
                tracing::warn!("Could not fetch diff for PR #{}: {e}", pr.number);
                continue;
            }
        };

        // PR size warnings
        let size = compute_diff_size(&diff);
        if !json {
            print_size_warning(pr.number, &size);
        }

        // AI inline review
        let user_prompt = inline_review::build_user_prompt(
            &pr.title,
            pr.body.as_deref().unwrap_or(""),
            &diff,
        );

        let result: InlineReviewResult = match ai.review(inline_review::SYSTEM, &user_prompt).await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to review PR #{}: {e:#}", pr.number);
                continue;
            }
        };

        // Print results
        if !json {
            print_review(pr.number, &pr.title, &result, &size, args.apply);
        }

        // Apply: post review on GitHub
        if args.apply && !result.comments.is_empty() {
            let comments: Vec<(String, u64, String)> = result
                .comments
                .iter()
                .map(|c| {
                    let body = format!("**[{}]** {}", c.severity.to_uppercase(), c.body);
                    (c.path.clone(), c.line, body)
                })
                .collect();

            let review_body = format!(
                "{}## Inline Code Review\n\n{}\n\n{}\n\n{}",
                config.branding.header(),
                result.summary,
                format_size_summary(&size),
                config.branding.footer("Reviewed"),
            );

            match gh.submit_review(pr.number, &review_body, &comments).await {
                Ok(()) => info!("Posted review on PR #{} ({} comments)", pr.number, comments.len()),
                Err(e) => tracing::error!("Failed to post review on PR #{}: {e:#}", pr.number),
            }
        }

        results.push(ReviewOutput {
            pr_number: pr.number,
            title: pr.title.clone(),
            applied: args.apply,
            additions: size.additions,
            deletions: size.deletions,
            files_changed: size.files_changed,
            review: result,
        });
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }

    Ok(())
}

// --- PR Size ---

pub struct DiffSize {
    pub additions: usize,
    pub deletions: usize,
    pub files_changed: usize,
    pub large_files: Vec<(String, usize)>, // (path, lines_changed)
}

impl DiffSize {
    pub fn total_lines(&self) -> usize {
        self.additions + self.deletions
    }

    pub fn is_large(&self) -> bool {
        self.total_lines() > 500 || self.files_changed > 20
    }

    pub fn is_huge(&self) -> bool {
        self.total_lines() > 1500 || self.files_changed > 50
    }
}

pub fn compute_diff_size(diff: &str) -> DiffSize {
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut files: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut current_file = String::new();

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            // Extract filename: diff --git a/path b/path
            if let Some(b_path) = line.split(" b/").last() {
                current_file = b_path.to_string();
            }
        } else if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
            *files.entry(current_file.clone()).or_default() += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
            *files.entry(current_file.clone()).or_default() += 1;
        }
    }

    let mut large_files: Vec<(String, usize)> = files
        .into_iter()
        .filter(|(_, lines)| *lines > 100)
        .collect();
    large_files.sort_by(|a, b| b.1.cmp(&a.1));

    DiffSize {
        additions,
        deletions,
        files_changed: diff
            .lines()
            .filter(|l| l.starts_with("diff --git"))
            .count(),
        large_files,
    }
}

fn print_size_warning(number: u64, size: &DiffSize) {
    if size.is_huge() {
        println!(
            "  ⚠ PR #{number}: VERY LARGE — +{} -{} across {} files. Consider splitting.",
            size.additions, size.deletions, size.files_changed,
        );
    } else if size.is_large() {
        println!(
            "  ⚠ PR #{number}: Large PR — +{} -{} across {} files.",
            size.additions, size.deletions, size.files_changed,
        );
    }
    for (path, lines) in &size.large_files {
        println!("    └ {path}: {lines} lines changed");
    }
}

fn format_size_summary(size: &DiffSize) -> String {
    let label = if size.is_huge() {
        "🔴 **Very Large PR**"
    } else if size.is_large() {
        "🟡 **Large PR**"
    } else {
        "🟢 **Normal size**"
    };
    format!(
        "{label} — +{} -{} across {} files ({} total lines)",
        size.additions,
        size.deletions,
        size.files_changed,
        size.total_lines(),
    )
}

fn print_review(number: u64, title: &str, result: &InlineReviewResult, size: &DiffSize, applied: bool) {
    let status = if applied { "APPLIED" } else { "DRY-RUN" };
    let truncated_title = if title.len() > 50 {
        format!("{}…", &title[..49])
    } else {
        title.to_string()
    };

    println!(
        "  [{status}] #{number} {truncated_title} — {} comments, +{} -{} ({} files)",
        result.comments.len(),
        size.additions,
        size.deletions,
        size.files_changed,
    );

    if !result.comments.is_empty() {
        for c in &result.comments {
            let severity_icon = match c.severity.as_str() {
                "error" => "🔴",
                "warning" => "🟡",
                _ => "🔵",
            };
            println!("    {} {}:{} — {}", severity_icon, c.path, c.line, c.body);
        }
    }

    if !result.summary.is_empty() && result.summary != "No issues found." {
        println!("  Summary: {}", result.summary);
    }
}
