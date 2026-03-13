use anyhow::Result;
use serde::Serialize;
use tracing::info;

use super::truncate;
use crate::cli::QueueArgs;
use crate::config::Config;
use crate::db::pulls::PullRequest;
use crate::db::Database;
use crate::export::ExportManager;
use crate::github::Client as GhClient;

struct ScoredPr {
    pr: PullRequest,
    score: i32,
    breakdown: Vec<String>,
}

#[derive(Serialize)]
struct QueueOutput {
    pr_number: u64,
    title: String,
    score: i32,
    breakdown: Vec<String>,
}

pub async fn run(
    config: &Config,
    db: &Database,
    _gh: &GhClient,
    args: &QueueArgs,
    json: bool,
    _exporter: Option<&ExportManager>,
) -> Result<()> {
    let pulls = db.get_open_pulls()?;

    if pulls.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No open PRs in queue.");
        }
        return Ok(());
    }

    let mut scored: Vec<ScoredPr> = pulls.into_iter().map(|pr| score_pr(&pr, config)).collect();
    scored.sort_by(|a, b| b.score.cmp(&a.score));

    if !json {
        println!("Merge Queue ({} PRs):", scored.len());
        println!("{:<6} {:<8} {:<60} {}", "#", "Score", "Title", "Breakdown");
        println!("{}", "-".repeat(90));

        for item in &scored {
            println!(
                "#{:<5} {:<8} {:<60} {}",
                item.pr.number,
                item.score,
                truncate(&item.pr.title, 58),
                item.breakdown.join(", "),
            );
        }
    }

    if args.apply {
        if let Some(top) = scored.first() {
            if top.score >= config.queue.merge_threshold {
                info!("Would merge PR #{} (score: {})", top.pr.number, top.score);
                if !json {
                    println!(
                        "\nMerging PR #{} (score: {} >= threshold: {})",
                        top.pr.number, top.score, config.queue.merge_threshold
                    );
                    // TODO: actual merge via GitHub API
                    println!("Warning: Merge not yet implemented. Coming in M4.");
                }
            } else if !json {
                println!(
                    "\nTop PR #{} has score {} (below threshold: {}). Not merging.",
                    top.pr.number, top.score, config.queue.merge_threshold
                );
            }
        }
    }

    if json {
        let results: Vec<QueueOutput> = scored
            .iter()
            .map(|item| QueueOutput {
                pr_number: item.pr.number,
                title: item.pr.title.clone(),
                score: item.score,
                breakdown: item.breakdown.clone(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&results)?);
    }

    Ok(())
}

fn score_pr(pr: &PullRequest, _config: &Config) -> ScoredPr {
    let (score, breakdown) = super::pr_health::score_pr(pr);
    ScoredPr {
        pr: pr.clone(),
        score,
        breakdown,
    }
}
