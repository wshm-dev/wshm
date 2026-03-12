use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::cli::QueueArgs;
use crate::config::Config;
use crate::db::pulls::PullRequest;
use crate::db::Database;
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
    let mut score = 0i32;
    let mut breakdown = Vec::new();

    // CI passing
    if pr.ci_status.as_deref() == Some("success") {
        score += 10;
        breakdown.push("CI:+10".to_string());
    }

    // Conflicts
    if pr.mergeable == Some(false) {
        score -= 10;
        breakdown.push("conflict:-10".to_string());
    } else if pr.mergeable == Some(true) {
        score += 2;
        breakdown.push("mergeable:+2".to_string());
    }

    // Age bonus (1 per day, max 10)
    if let Ok(created) = pr.created_at.parse::<chrono::DateTime<chrono::Utc>>() {
        let days = chrono::Utc::now().signed_duration_since(created).num_days();
        let age_bonus = days.min(10) as i32;
        if age_bonus > 0 {
            score += age_bonus;
            breakdown.push(format!("age:+{age_bonus}"));
        }
    }

    // Has linked issue in body
    if let Some(ref body) = pr.body {
        if body.contains("fixes #") || body.contains("closes #") || body.contains("resolves #") {
            score += 3;
            breakdown.push("linked:+3".to_string());
        }
    }

    ScoredPr {
        pr: pr.clone(),
        score,
        breakdown,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
