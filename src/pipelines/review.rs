use crate::db::backend::DatabaseBackend;
use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::ai::backend::AiBackend;
use crate::ai::prompts::inline_review;
use crate::ai::schemas::{InlineComment, InlineReviewResult, ReviewStats};
use crate::cli::ReviewArgs;
use crate::config::Config;
use crate::git_provider::Forge as GhClient;
use crate::pipelines::truncate;

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

/// What the inline review did for one PR. A failure inside the review (diff
/// fetch, AI call) is reported here rather than as an `Err`, so callers
/// keep the historical "warn and continue" behaviour while still knowing
/// it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewOutcome {
    Reviewed { comments: u32, posted: bool },
    Skipped(String),
    Failed(String),
}

/// Minimum number of files to trigger per-file chunking instead of full-diff mode.
const CHUNK_THRESHOLD_FILES: usize = 2;

/// Review the PR(s) of `args`. Per-PR failures are logged and do not fail
/// the pass; they are returned as outcomes so the daemon's pipeline step
/// can record them.
pub async fn run(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    args: &ReviewArgs,
    json: bool,
) -> Result<Vec<(u64, ReviewOutcome)>> {
    let ai = AiBackend::from_config(config, &config.ai.model)?;
    let guard_cfg = crate::ai::guard::load(config, db);

    // Skills — standing instructions configured in Settings, same mechanism
    // triage/PR-analysis already use. Fetched once per run (constant across
    // every PR reviewed this pass) and appended to every prompt below.
    let skills = crate::config::load_skills(db);
    let skills_prompt = crate::config::skills_prompt(&skills, "review");

    let pulls = if let Some(number) = args.pr {
        match db.get_pull(number)? {
            Some(pr) => vec![pr],
            None => {
                if json {
                    println!("[]");
                } else {
                    println!("PR #{number} not found in cache. Run `wshm sync` first.");
                }
                return Ok(vec![(
                    number,
                    ReviewOutcome::Skipped("PR not found in cache".into()),
                )]);
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
        return Ok(Vec::new());
    }

    let mut results: Vec<ReviewOutput> = Vec::with_capacity(pulls.len());
    let mut outcomes: Vec<(u64, ReviewOutcome)> = Vec::with_capacity(pulls.len());

    for pr in &pulls {
        info!("Reviewing PR #{}: {}", pr.number, pr.title);

        // Fetch the raw diff
        let diff = match gh.fetch_pr_diff(pr.number).await {
            Ok(d) if !d.is_empty() => d,
            Ok(_) => {
                if !json {
                    println!("  PR #{}: empty diff, skipping.", pr.number);
                }
                outcomes.push((pr.number, ReviewOutcome::Skipped("empty diff".into())));
                continue;
            }
            Err(e) => {
                tracing::warn!("Could not fetch diff for PR #{}: {e}", pr.number);
                outcomes.push((
                    pr.number,
                    ReviewOutcome::Failed(format!("could not fetch the diff: {e}")),
                ));
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
            review_per_file(&ai, &pr.title, pr_body, &file_chunks, &skills_prompt).await
        } else {
            // Small PR: send full diff in one shot
            let mut user_prompt = inline_review::build_user_prompt(&pr.title, pr_body, &diff);
            if !skills_prompt.is_empty() {
                user_prompt.push_str(&skills_prompt);
            }
            ai.complete(inline_review::SYSTEM, &user_prompt).await
        };

        let mut result = match result {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to review PR #{}: {e:#}", pr.number);
                outcomes.push((pr.number, ReviewOutcome::Failed(format!("{e:#}"))));
                continue;
            }
        };

        // Output validation (#235): comments must target a line of the new
        // side of the diff; text loses mentions, links and HTML.
        let dropped = crate::ai::output::validate_review(&mut result, &diff);
        if dropped > 0 {
            info!(
                "PR #{}: {dropped} review comment(s) dropped (line not in the diff or invalid)",
                pr.number
            );
        }

        // Injection detection over the PR text and the added lines (#235).
        let pr_text = format!("{}\n\n{pr_body}", pr.title);
        let assessment = crate::ai::guard::assess_texts(
            &guard_cfg,
            &[
                ("pull request", pr_text.as_str()),
                ("pull request diff", diff.as_str()),
            ],
        );
        crate::ai::guard::record(db, "pull", pr.number, "review", &assessment);
        let note = if assessment.suspicious && guard_cfg.maintainer_note {
            format!(
                "\n\n{}",
                crate::ai::guard::maintainer_note("pull request", &assessment)
            )
        } else {
            String::new()
        };

        // Print results
        if !json {
            print_review(pr.number, &pr.title, &result, &size, args.apply);
        }

        // Apply: post review on GitHub
        let mut posted_to_github = false;
        if args.apply && !result.comments.is_empty() {
            let comments: Vec<(String, u64, String)> = result
                .comments
                .iter()
                .map(|c| {
                    let body = truncate(&format_github_comment(c), 2000);
                    (c.path.clone(), c.line, body)
                })
                .collect();

            let review_body = format!(
                "{}## Automated Code Review

{}

{}

{}{note}

{}",
                config.branding.header(),
                truncate(&result.summary, 500),
                format_stats_summary(&result.stats),
                format_size_summary(&size),
                config.branding.footer("Reviewed"),
            );

            match gh.submit_review(pr.number, &review_body, &comments).await {
                Ok(()) => {
                    info!(
                        "Posted review on PR #{} ({} comments)",
                        pr.number,
                        comments.len()
                    );
                    posted_to_github = true;
                }
                Err(e) => tracing::error!("Failed to post review on PR #{}: {e:#}", pr.number),
            }
        }

        // Store regardless of apply mode, so the /review page always
        // reflects the latest computed review even when it wasn't (or
        // couldn't be) posted to GitHub.
        if let Err(e) = db.upsert_pr_review(&crate::db::reviews::PrReviewRow {
            pr_number: pr.number,
            result: result.clone(),
            posted_to_github,
            content_hash: None,
            reviewed_at: chrono::Utc::now().to_rfc3339(),
        }) {
            tracing::warn!("Failed to store review for PR #{}: {e}", pr.number);
        }

        outcomes.push((
            pr.number,
            ReviewOutcome::Reviewed {
                comments: result.comments.len() as u32,
                posted: posted_to_github,
            },
        ));
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

    Ok(outcomes)
}

/// The inline review of one PR, computed but not posted (the pipeline
/// `review` step with `mode = "collect"`: the `verify` step posts the
/// confirmed findings). Same prompts and output validation as [`run`];
/// `extra_context` (already wrapped as untrusted blocks, e.g. CLAUDE.md /
/// REVIEW.md and the code around the changed lines) is appended to every
/// prompt. At most `max_calls` AI calls are made (per-file mode reviews
/// the first files only).
pub struct Collected {
    pub result: InlineReviewResult,
    /// Files not reviewed because of `max_calls`.
    pub files_left_out: usize,
    /// The PR text or diff looks like a prompt injection attempt and
    /// mutating actions are disabled for it.
    pub blocked: bool,
}

/// Files of `diff` the per-file review looks at (all of them in one call
/// when there are fewer than two).
fn review_chunks(diff: &str) -> Option<Vec<(String, String)>> {
    let chunks = inline_review::split_diff_by_file(diff);
    (chunks.len() >= CHUNK_THRESHOLD_FILES).then(|| {
        chunks
            .into_iter()
            .filter(|(p, _)| !should_skip_file(p))
            .collect()
    })
}

/// AI calls [`collect`] makes for `diff` without a limit.
pub fn review_calls_needed(diff: &str) -> u32 {
    review_chunks(diff).map_or(1, |c| c.len() as u32)
}

pub async fn collect(
    config: &Config,
    db: &dyn DatabaseBackend,
    ai: &AiBackend,
    number: u64,
    diff: &str,
    extra_context: &str,
    max_calls: u32,
) -> Result<std::result::Result<Collected, String>> {
    let Some(pr) = db.get_pull(number)? else {
        return Ok(Err("PR not found in cache".into()));
    };
    if diff.trim().is_empty() {
        return Ok(Err("empty diff".into()));
    }
    if max_calls == 0 {
        return Ok(Err("AI call budget reached".into()));
    }
    let guard_cfg = crate::ai::guard::load(config, db);
    let skills = crate::config::load_skills(db);
    let mut suffix = crate::config::skills_prompt(&skills, "review");
    suffix.push_str(extra_context);
    let pr_body = pr.body.as_deref().unwrap_or("");
    let mut files_left_out = 0;
    let mut result = match review_chunks(diff) {
        Some(mut chunks) => {
            let max = max_calls as usize;
            files_left_out = chunks.len().saturating_sub(max);
            chunks.truncate(max);
            review_per_file(ai, &pr.title, pr_body, &chunks, &suffix).await?
        }
        None => {
            let mut user_prompt = inline_review::build_user_prompt(&pr.title, pr_body, diff);
            user_prompt.push_str(&suffix);
            ai.complete(inline_review::SYSTEM, &user_prompt).await?
        }
    };
    let dropped = crate::ai::output::validate_review(&mut result, diff);
    if dropped > 0 {
        info!(
            "PR #{number}: {dropped} review comment(s) dropped (line not in the diff or invalid)"
        );
    }
    let pr_text = format!("{}\n\n{pr_body}", pr.title);
    let assessment = crate::ai::guard::assess_texts(
        &guard_cfg,
        &[
            ("pull request", pr_text.as_str()),
            ("pull request diff", diff),
        ],
    );
    crate::ai::guard::record(db, "pull", number, "review", &assessment);
    Ok(Ok(Collected {
        result,
        files_left_out,
        blocked: assessment.blocked,
    }))
}

/// Review each file separately for better context and fewer truncation issues.
async fn review_per_file(
    ai: &AiBackend,
    pr_title: &str,
    pr_body: &str,
    file_chunks: &[(String, String)],
    skills_prompt: &str,
) -> Result<InlineReviewResult> {
    let mut all_comments: Vec<InlineComment> = Vec::with_capacity(file_chunks.len() * 2);
    let mut summaries: Vec<String> = Vec::with_capacity(file_chunks.len());
    let mut stats = ReviewStats::default();

    for (file_path, file_diff) in file_chunks {
        // Skip non-code files
        if should_skip_file(file_path) {
            continue;
        }

        let mut user_prompt =
            inline_review::build_file_prompt(pr_title, pr_body, file_path, file_diff);
        if !skills_prompt.is_empty() {
            user_prompt.push_str(skills_prompt);
        }

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
        summaries.join(
            "
",
        )
    };
    Ok(InlineReviewResult {
        comments: all_comments,
        summary,
        stats,
    })
}

/// Format an inline comment for GitHub, including suggestion blocks.
pub(crate) fn format_github_comment(c: &InlineComment) -> String {
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

    let mut body = format!(
        "{severity_badge} | {category_badge}

{}",
        c.body
    );

    if let Some(ref suggestion) = c.suggestion {
        if !suggestion.is_empty() {
            body.push_str(&format!(
                "

```suggestion
{suggestion}
```"
            ));
        }
    }

    body
}

/// Files to skip during review (generated, config, non-code).
pub(crate) fn should_skip_file(path: &str) -> bool {
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
    large_files.sort_by_key(|b| std::cmp::Reverse(b.1));

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
