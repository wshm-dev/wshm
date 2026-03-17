use anyhow::Result;
use serde::Serialize;
use tracing::info;

use super::truncate;
use crate::ai::backend::AiBackend;
use crate::ai::prompts::inline_review;
use crate::ai::schemas::{InlineComment, InlineReviewResult, ReviewStats};
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

/// Minimum number of files to trigger per-file chunking instead of full-diff mode.
const CHUNK_THRESHOLD_FILES: usize = 2;

pub async fn run(
    config: &Config,
    db: &Database,
    gh: &GhClient,
    args: &ReviewArgs,
    json: bool,
) -> Result<()> {
    let ai = AiBackend::from_config(config, &config.ai.model)?;

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

    let mut results: Vec<ReviewOutput> = Vec::with_capacity(pulls.len());

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

        // Split diff into per-file chunks
        let file_chunks = inline_review::split_diff_by_file(&diff);
        let pr_body = pr.body.as_deref().unwrap_or("");

        let result = if file_chunks.len() >= CHUNK_THRESHOLD_FILES {
            // Per-file review: better context, no truncation issues
            review_per_file(&ai, &pr.title, pr_body, &file_chunks).await
        } else {
            // Small PR: send full diff in one shot
            let user_prompt = inline_review::build_user_prompt(&pr.title, pr_body, &diff);
            ai.complete(inline_review::SYSTEM, &user_prompt).await
        };

        let result = match result {
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
                    let body = format_github_comment(c);
                    (c.path.clone(), c.line, body)
                })
                .collect();

            let review_body = format!(
                "{}## 🔎 Automated Code Review\n\n{}\n\n{}\n\n{}\n\n{}",
                config.branding.header(),
                result.summary,
                format_stats_summary(&result.stats),
                format_size_summary(&size),
                config.branding.footer("Reviewed"),
            );

            match gh.submit_review(pr.number, &review_body, &comments).await {
                Ok(()) => info!(
                    "Posted review on PR #{} ({} comments)",
                    pr.number,
                    comments.len()
                ),
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

/// Review each file separately for better context and fewer truncation issues.
async fn review_per_file(
    ai: &AiBackend,
    pr_title: &str,
    pr_body: &str,
    file_chunks: &[(String, String)],
) -> Result<InlineReviewResult> {
    let mut all_comments: Vec<InlineComment> = Vec::with_capacity(file_chunks.len() * 2);
    let mut summaries: Vec<String> = Vec::with_capacity(file_chunks.len());
    let mut stats = ReviewStats::default();

    for (file_path, file_diff) in file_chunks {
        // Skip non-code files
        if should_skip_file(file_path) {
            continue;
        }

        let user_prompt = inline_review::build_file_prompt(pr_title, pr_body, file_path, file_diff);

        match ai
            .complete::<InlineReviewResult>(inline_review::SYSTEM, &user_prompt)
            .await
        {
            Ok(result) => {
                stats.errors += result.stats.errors;
                stats.warnings += result.stats.warnings;
                stats.infos += result.stats.infos;

                if !result.summary.is_empty()
                    && result.summary != "No issues found."
                    && !result.comments.is_empty()
                {
                    summaries.push(format!("**{}**: {}", file_path, result.summary));
                }

                all_comments.extend(result.comments);
            }
            Err(e) => {
                tracing::warn!("Failed to review {file_path}: {e:#}");
            }
        }
    }

    // Recompute stats from actual comments if AI stats seem off
    if stats.errors + stats.warnings + stats.infos != all_comments.len() {
        stats = ReviewStats::default();
        for c in &all_comments {
            match c.severity.as_str() {
                "error" => stats.errors += 1,
                "warning" => stats.warnings += 1,
                _ => stats.infos += 1,
            }
        }
    }

    let summary = if summaries.is_empty() {
        "No issues found.".to_string()
    } else {
        summaries.join("\n")
    };

    Ok(InlineReviewResult {
        comments: all_comments,
        summary,
        stats,
    })
}

/// Format an inline comment for GitHub, including suggestion blocks.
fn format_github_comment(c: &InlineComment) -> String {
    let severity_badge = match c.severity.as_str() {
        "error" => "🔴 **ERROR**",
        "warning" => "🟡 **WARNING**",
        _ => "🔵 **INFO**",
    };

    let category_badge = match c.category.as_str() {
        "security" => "🔒 Security",
        "bug" => "🐛 Bug",
        "perf" => "⚡ Performance",
        "race-condition" => "🏁 Race Condition",
        "resource-leak" => "💧 Resource Leak",
        "error-handling" => "⚠️ Error Handling",
        "logic" => "🧠 Logic",
        other => other,
    };

    let mut body = format!("{severity_badge} | {category_badge}\n\n{}", c.body);

    if let Some(ref suggestion) = c.suggestion {
        if !suggestion.is_empty() {
            body.push_str(&format!("\n\n```suggestion\n{suggestion}\n```"));
        }
    }

    body
}

/// Files to skip during review (generated, config, non-code).
fn should_skip_file(path: &str) -> bool {
    let skip_extensions = [
        ".lock", ".sum", ".min.js", ".min.css", ".map", ".svg", ".png", ".jpg", ".ico", ".woff",
        ".woff2", ".ttf", ".eot", ".pdf",
    ];

    let skip_paths = [
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "Cargo.lock",
        "go.sum",
        "composer.lock",
        "Gemfile.lock",
        "poetry.lock",
        "Pipfile.lock",
        ".gitignore",
        ".gitattributes",
    ];

    if skip_paths.iter().any(|p| path.ends_with(p)) {
        return true;
    }

    if skip_extensions.iter().any(|ext| path.ends_with(ext)) {
        return true;
    }

    false
}

// --- PR Size ---

pub struct DiffSize {
    pub additions: usize,
    pub deletions: usize,
    pub files_changed: usize,
    pub large_files: Vec<(String, usize)>,
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
    let mut files: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut current_file = "";

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            if let Some(b_path) = line.split(" b/").last() {
                current_file = b_path;
            }
        } else if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
            *files.entry(current_file).or_default() += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
            *files.entry(current_file).or_default() += 1;
        }
    }

    let mut large_files: Vec<(String, usize)> = files
        .into_iter()
        .filter(|(_, lines)| *lines > 100)
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    large_files.sort_by(|a, b| b.1.cmp(&a.1));

    DiffSize {
        additions,
        deletions,
        files_changed: diff.lines().filter(|l| l.starts_with("diff --git")).count(),
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

fn format_stats_summary(stats: &ReviewStats) -> String {
    let total = stats.errors + stats.warnings + stats.infos;
    if total == 0 {
        return "✅ **No issues found**".to_string();
    }

    let mut parts = Vec::new();
    if stats.errors > 0 {
        parts.push(format!(
            "🔴 {} error{}",
            stats.errors,
            if stats.errors > 1 { "s" } else { "" }
        ));
    }
    if stats.warnings > 0 {
        parts.push(format!(
            "🟡 {} warning{}",
            stats.warnings,
            if stats.warnings > 1 { "s" } else { "" }
        ));
    }
    if stats.infos > 0 {
        parts.push(format!(
            "🔵 {} info{}",
            stats.infos,
            if stats.infos > 1 { "s" } else { "" }
        ));
    }

    format!(
        "**{} issue{} found:** {}",
        total,
        if total > 1 { "s" } else { "" },
        parts.join(" · ")
    )
}

fn print_review(
    number: u64,
    title: &str,
    result: &InlineReviewResult,
    size: &DiffSize,
    applied: bool,
) {
    let status = if applied { "APPLIED" } else { "DRY-RUN" };
    let truncated_title = truncate(title, 50);

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
            let cat = match c.category.as_str() {
                "security" => "SEC",
                "bug" => "BUG",
                "perf" => "PERF",
                "race-condition" => "RACE",
                "resource-leak" => "LEAK",
                "error-handling" => "ERR",
                "logic" => "LOGIC",
                other => other,
            };
            println!(
                "    {} [{}] {}:{} — {}",
                severity_icon, cat, c.path, c.line, c.body
            );
            if let Some(ref suggestion) = c.suggestion {
                if !suggestion.is_empty() {
                    // Show first line of suggestion
                    let first_line = suggestion.lines().next().unwrap_or("");
                    println!("      💡 {first_line}");
                }
            }
        }
    }

    // Stats
    let stats = &result.stats;
    let total = stats.errors + stats.warnings + stats.infos;
    if total > 0 {
        println!(
            "  Stats: {} error(s), {} warning(s), {} info(s)",
            stats.errors, stats.warnings, stats.infos,
        );
    }

    if !result.summary.is_empty() && result.summary != "No issues found." {
        println!("  Summary: {}", result.summary);
    }
}
