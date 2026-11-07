use anyhow::Result;
use serde::Serialize;
use tracing::{info, warn};

use super::truncate;
use crate::cli::QueueArgs;
use crate::config::Config;
use crate::db::backend::DatabaseBackend;
use crate::db::pulls::PullRequest;
use crate::export::ExportManager;
use crate::git_provider::{Capability, Forge as GhClient, MergeOutcome};

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
    db: &dyn DatabaseBackend,
    gh: &GhClient,
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
    scored.sort_by_key(|b| std::cmp::Reverse(b.score));

    if !json {
        println!("Merge Queue ({} PRs):", scored.len());
        println!("{:<6} {:<8} {:<60} Breakdown", "#", "Score", "Title");
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
            // A PR flagged as possible prompt injection (#235) is never
            // merged automatically, whatever its score.
            if crate::ai::guard::is_flagged(db, "pull", top.pr.number) {
                info!(
                    "Top PR #{} is flagged as possible prompt injection: not merging",
                    top.pr.number
                );
                if !json {
                    println!(
                        "\nTop PR #{} is flagged as possible prompt injection. Not merging.",
                        top.pr.number
                    );
                }
            } else if top.score >= config.queue.merge_threshold {
                if !json {
                    println!(
                        "\nMerging PR #{} (score: {} >= threshold: {})",
                        top.pr.number, top.score, config.queue.merge_threshold
                    );
                }
                let msg = merge_top(config, gh, &top.pr).await;
                if !json {
                    println!("{msg}");
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

/// Merge the top PR on a forge that supports it. Never merges a PR with
/// conflicts or failed CI; with CI still running the forge merges it once
/// CI succeeds. Pinned to the head SHA that was scored. Returns the line
/// to print.
async fn merge_top(config: &Config, gh: &GhClient, pr: &PullRequest) -> String {
    if !gh.supports(Capability::Merge) {
        return format!(
            "Warning: merging is not supported on {} yet. Not merging.",
            gh.provider_name()
        );
    }
    if pr.mergeable == Some(false) {
        return format!("PR #{} has conflicts. Not merging.", pr.number);
    }
    let ci = pr.ci_status.as_deref();
    if ci == Some("failure") {
        return format!("PR #{} has failing CI. Not merging.", pr.number);
    }
    let squash = config.queue.strategy.eq_ignore_ascii_case("squash");
    let when_ci_passes = ci == Some("pending");
    match gh
        .merge_pr(pr.number, pr.head_sha.as_deref(), squash, when_ci_passes)
        .await
    {
        Ok(MergeOutcome::Merged) => {
            info!("Merged PR #{}", pr.number);
            format!("Merged PR #{}.", pr.number)
        }
        Ok(MergeOutcome::Scheduled) => {
            info!("PR #{} set to merge when CI succeeds", pr.number);
            format!("PR #{} will be merged when CI succeeds.", pr.number)
        }
        Err(e) => {
            warn!("Failed to merge PR #{}: {e:#}", pr.number);
            format!("Failed to merge PR #{}: {e:#}", pr.number)
        }
    }
}

fn score_pr(pr: &PullRequest, config: &Config) -> ScoredPr {
    let (score, breakdown) = super::pr_health::score_pr_with(pr, &config.scoring.pr);
    ScoredPr {
        pr: pr.clone(),
        score,
        breakdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_provider::gitlab::GitLabProvider;
    use crate::git_provider::http::{Auth, Rest};
    use crate::git_provider::mock::{MockServer, Reply};
    use serde_json::json;

    fn pr(mergeable: Option<bool>, ci: Option<&str>) -> PullRequest {
        PullRequest {
            number: 5,
            title: "t".into(),
            body: None,
            state: "open".into(),
            labels: vec![],
            author: None,
            head_sha: Some("abc".into()),
            base_sha: None,
            head_ref: None,
            base_ref: None,
            mergeable,
            ci_status: ci.map(String::from),
            created_at: String::new(),
            updated_at: String::new(),
            review_decision: None,
            review_decision_at: None,
            reactions_plus1: 0,
        }
    }

    fn gitlab(server: &MockServer) -> GitLabProvider {
        GitLabProvider::with_rest(
            Rest::new("GitLab", Auth::Header("Authorization", "Bearer t".into())).unwrap(),
            &server.url(),
            "g/p",
            "<!-- wshm -->".into(),
        )
    }

    const MERGE: &str = "/api/v4/projects/g%2Fp/merge_requests/5/merge";

    #[tokio::test]
    async fn merges_on_gitlab_pinned_to_the_scored_sha() {
        let server = MockServer::start().await;
        server.on("PUT", MERGE, Reply::json(json!({"state": "merged"})));
        let mut config = Config::default();
        config.queue.strategy = "squash".into();
        let msg = merge_top(&config, &gitlab(&server), &pr(Some(true), Some("success"))).await;
        assert_eq!(msg, "Merged PR #5.");
        let body = server.requests()[0].json();
        assert_eq!(body, json!({"sha": "abc", "squash": true}));
    }

    #[tokio::test]
    async fn running_ci_schedules_the_merge() {
        let server = MockServer::start().await;
        server.on(
            "PUT",
            MERGE,
            Reply::json(json!({"state": "opened", "merge_when_pipeline_succeeds": true})),
        );
        let msg = merge_top(
            &Config::default(),
            &gitlab(&server),
            &pr(None, Some("pending")),
        )
        .await;
        assert!(msg.contains("when CI succeeds"), "{msg}");
        let body = server.requests()[0].json();
        assert_eq!(body["merge_when_pipeline_succeeds"], true);
        assert_eq!(body["auto_merge"], true);
        assert_eq!(body["squash"], false);
    }

    #[tokio::test]
    async fn never_merges_conflicts_or_failed_ci_and_reports_errors() {
        let server = MockServer::start().await;
        let g = gitlab(&server);
        let c = Config::default();
        assert!(merge_top(&c, &g, &pr(Some(false), None))
            .await
            .contains("conflicts"));
        assert!(merge_top(&c, &g, &pr(Some(true), Some("failure")))
            .await
            .contains("failing CI"));
        assert!(server.requests().is_empty());

        // Head moved since scoring: GitLab refuses with 409.
        server.on(
            "PUT",
            MERGE,
            Reply::json(json!({"message": "SHA does not match HEAD of source branch"})).status(409),
        );
        let msg = merge_top(&c, &g, &pr(Some(true), None)).await;
        assert!(
            msg.starts_with("Failed to merge PR #5") && msg.contains("409"),
            "{msg}"
        );

        // A forge without merge support only reports.
        let rec = crate::git_provider::recording::RecordingForge::default();
        assert!(merge_top(&c, &rec, &pr(Some(true), None))
            .await
            .contains("not supported"));
    }
}
