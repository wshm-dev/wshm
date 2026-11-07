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
            } else if top.score >= config.queue.merge_threshold && !config.queue.auto_merge {
                info!("Would merge PR #{} (score: {})", top.pr.number, top.score);
                if !json {
                    println!(
                        "\nWould merge PR #{} (score: {} >= threshold: {}). Set [queue] auto_merge = true to merge.",
                        top.pr.number, top.score, config.queue.merge_threshold
                    );
                }
            } else if top.score >= config.queue.merge_threshold {
                if !json {
                    println!(
                        "\nMerging PR #{} (score: {} >= threshold: {})",
                        top.pr.number, top.score, config.queue.merge_threshold
                    );
                }
                let msg = merge_top(config, db, gh, &top.pr).await;
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

/// Merge-queue policy shared by every forge: why `scored` (the PR as it
/// was scored, from the DB) may not be merged automatically right now, or
/// the live head SHA to pin the merge to. All of these must hold, whatever
/// the score (labels set by the AI never stand in for an approval):
/// `[queue] auto_merge` on, forge merge support, not flagged by the
/// injection guard, still open with the scored head, no conflicts, CI
/// green now (pending: wait for the next run, never scheduled), and
/// `[queue] required_approvals` approvals from members with write access
/// (the author, wshm's own account and bots not counting).
pub async fn merge_gate(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    scored: &PullRequest,
) -> std::result::Result<String, String> {
    let n = scored.number;
    if !config.queue.auto_merge {
        return Err("[queue] auto_merge is off".into());
    }
    if !gh.supports(Capability::Merge) {
        return Err(format!(
            "merging is not supported on {}",
            gh.provider_name()
        ));
    }
    if crate::ai::guard::is_flagged(db, "pull", n) {
        return Err("flagged as possible prompt injection".into());
    }
    let Some(scored_sha) = scored.head_sha.as_deref().filter(|s| !s.is_empty()) else {
        return Err("no head SHA was scored".into());
    };
    let live = match gh.fetch_pull(n).await {
        Ok(Some(pr)) => pr,
        Ok(None) => return Err("not found on the forge".into()),
        Err(e) => return Err(format!("cannot read its live state: {e:#}")),
    };
    if live.state != "open" {
        return Err(format!("is {}", live.state));
    }
    if live.head_sha.as_deref() != Some(scored_sha) {
        return Err("new commits since it was scored: next run".into());
    }
    match live.mergeable {
        Some(true) => {}
        Some(false) => return Err("has conflicts".into()),
        None => return Err("mergeability not computed yet: next run".into()),
    }
    match live.ci_status.as_deref() {
        Some("success") => {}
        Some("pending") => return Err("CI is still running: next run".into()),
        Some(other) => return Err(format!("CI is {other}")),
        None => return Err("no CI status (green CI is required)".into()),
    }
    let required = config.queue.required_approvals;
    if required > 0 {
        let have = qualified_approvals(gh, n, live.author.as_deref(), required).await?;
        if have < required {
            return Err(format!(
                "{have}/{required} approval(s) from members with write access"
            ));
        }
    }
    Ok(scored_sha.to_string())
}

/// Approvers counted by [`merge_gate`], up to `enough`.
async fn qualified_approvals(
    gh: &GhClient,
    number: u64,
    author: Option<&str>,
    enough: u32,
) -> std::result::Result<u32, String> {
    let approvers = gh
        .fetch_pr_approvers(number)
        .await
        .map_err(|e| format!("cannot read approvals: {e:#}"))?;
    let me = gh.self_username().await;
    let mut seen: Vec<String> = Vec::new();
    let mut count = 0;
    for login in approvers {
        let key = login.trim().to_ascii_lowercase();
        let is_author = author.is_some_and(|a| a.eq_ignore_ascii_case(&key));
        let is_me = me.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(&key));
        if key.is_empty() || is_author || is_me || looks_like_bot(&key) || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        match gh.is_collaborator(&login).await {
            Ok(true) => count += 1,
            Ok(false) => info!("PR #{number}: approval by '{login}' ignored (no write access)"),
            Err(e) => warn!("PR #{number}: cannot check approver '{login}': {e:#}"),
        }
        if count >= enough {
            break;
        }
    }
    Ok(count)
}

/// Bot naming conventions across forges (GitHub apps, GitLab access
/// tokens, ...). Adapters also drop accounts the forge marks as bots.
fn looks_like_bot(login: &str) -> bool {
    login.ends_with("[bot]")
        || login.ends_with("_bot")
        || login.ends_with("-bot")
        || login.contains("_bot_")
        || login == "ghost"
}

/// Merge the top PR when [`merge_gate`] allows it, pinned to the scored
/// head SHA. Returns the line to print.
pub async fn merge_top(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    pr: &PullRequest,
) -> String {
    let sha = match merge_gate(config, db, gh, pr).await {
        Ok(sha) => sha,
        Err(why) => {
            info!("PR #{} not merged: {why}", pr.number);
            return format!("PR #{} not merged: {why}.", pr.number);
        }
    };
    let squash = config.queue.strategy.eq_ignore_ascii_case("squash");
    match gh.merge_pr(pr.number, Some(&sha), squash).await {
        Ok(MergeOutcome::Merged) => {
            info!("Merged PR #{}", pr.number);
            format!("Merged PR #{}.", pr.number)
        }
        Ok(MergeOutcome::NotMerged) => {
            warn!("PR #{}: merge accepted but not merged", pr.number);
            format!(
                "PR #{} was not merged (the forge did not merge it).",
                pr.number
            )
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
    use crate::git_provider::GitProvider;
    use serde_json::{json, Value};

    const P: &str = "/api/v4/projects/g%2Fp";

    /// The PR as it was scored (DB copy), labelled high priority by the AI.
    fn scored() -> PullRequest {
        PullRequest {
            number: 5,
            title: "t".into(),
            body: None,
            state: "open".into(),
            labels: vec!["priority:critical".into()],
            author: Some("alice".into()),
            head_sha: Some("abc".into()),
            base_sha: None,
            head_ref: None,
            base_ref: None,
            mergeable: Some(true),
            ci_status: Some("success".into()),
            created_at: String::new(),
            updated_at: String::new(),
            review_decision: None,
            review_decision_at: None,
            reactions_plus1: 0,
        }
    }

    fn on(config: &mut Config) -> &mut Config {
        config.queue.auto_merge = true;
        config
    }

    fn live_mr(sha: &str, pipeline: &str, conflicts: bool) -> Value {
        json!({"iid": 5, "state": "opened", "sha": sha, "author": {"username": "alice"},
               "has_conflicts": conflicts, "detailed_merge_status": "mergeable",
               "head_pipeline": {"status": pipeline}})
    }

    /// Live MR + approvals: bob (Developer), the author, a project bot,
    /// wshm itself and carol (Reporter) approved.
    fn gitlab_with(server: &MockServer, mr: Value) -> GitLabProvider {
        server.on("GET", &format!("{P}/merge_requests/5"), Reply::json(mr));
        server.on(
            "GET",
            &format!("{P}/merge_requests/5/approvals"),
            Reply::json(json!({"approved_by": [
                {"user": {"username": "alice"}},
                {"user": {"username": "project_7_bot_3f2a"}},
                {"user": {"username": "helper", "bot": true}},
                {"user": {"username": "wshm-bot"}},
                {"user": {"username": "carol"}},
                {"user": {"username": "bob"}}
            ]})),
        );
        server.on(
            "GET",
            "/api/v4/user",
            Reply::json(json!({"username": "wshm-bot"})),
        );
        for (name, id, level) in [("bob", 1, 30), ("carol", 2, 20), ("alice", 3, 40)] {
            server.on(
                "GET",
                "/api/v4/users",
                Reply::json(json!([{"id": id, "username": name}]))
                    .when_query(&format!("username={name}")),
            );
            server.on(
                "GET",
                &format!("{P}/members/all/{id}"),
                Reply::json(json!({"access_level": level})),
            );
        }
        GitLabProvider::with_rest(
            Rest::new("GitLab", Auth::Header("Authorization", "Bearer t".into())).unwrap(),
            &server.url(),
            "g/p",
            "<!-- wshm -->".into(),
        )
    }

    fn puts(server: &MockServer) -> Vec<Value> {
        server
            .requests()
            .into_iter()
            .filter(|r| r.method == "PUT")
            .map(|r| r.json())
            .collect()
    }

    #[tokio::test]
    async fn merges_only_when_every_gate_holds() {
        let server = MockServer::start().await;
        server.on(
            "PUT",
            &format!("{P}/merge_requests/5/merge"),
            Reply::json(json!({"state": "merged"})),
        );
        let gl = gitlab_with(&server, live_mr("abc", "success", false));
        let db = crate::db::Database::open_memory().unwrap();
        let mut config = Config::default();
        config.queue.strategy = "squash".into();
        let msg = merge_top(on(&mut config), &db, &gl, &scored()).await;
        assert_eq!(msg, "Merged PR #5.");
        // Pinned to the scored head, merged now (never scheduled on CI).
        assert_eq!(puts(&server), vec![json!({"sha": "abc", "squash": true})]);
    }

    #[tokio::test]
    async fn auto_merge_is_off_by_default() {
        let server = MockServer::start().await;
        let gl = gitlab_with(&server, live_mr("abc", "success", false));
        let db = crate::db::Database::open_memory().unwrap();
        let config = Config::default();
        assert!(!config.queue.auto_merge);
        assert_eq!(config.queue.required_approvals, 1);
        let msg = merge_top(&config, &db, &gl, &scored()).await;
        assert!(msg.contains("auto_merge is off"), "{msg}");
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn refuses_pending_ci_moved_head_conflicts_and_flagged() {
        let db = crate::db::Database::open_memory().unwrap();
        let mut config = Config::default();
        let config = on(&mut config);
        for (mr, why) in [
            (live_mr("abc", "running", false), "CI is still running"),
            (live_mr("abc", "failed", false), "CI is failure"),
            (live_mr("def", "success", false), "new commits"),
            (live_mr("abc", "success", true), "conflicts"),
        ] {
            let server = MockServer::start().await;
            let gl = gitlab_with(&server, mr);
            let msg = merge_top(config, &db, &gl, &scored()).await;
            assert!(msg.contains(why), "{why}: {msg}");
            assert!(puts(&server).is_empty(), "{why}");
        }

        db.upsert_injection_flag(&crate::db::injection::InjectionFlagRow {
            kind: "pull".into(),
            number: 5,
            pipeline: "review".into(),
            score: 9,
            reasons: vec![],
            findings: vec![],
            blocked: true,
            detected_at: String::new(),
        })
        .unwrap();
        let server = MockServer::start().await;
        let gl = gitlab_with(&server, live_mr("abc", "success", false));
        let msg = merge_top(config, &db, &gl, &scored()).await;
        assert!(msg.contains("prompt injection"), "{msg}");
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn approvals_need_members_with_write_access() {
        let db = crate::db::Database::open_memory().unwrap();
        let mut config = Config::default();
        // Only bob counts: author, bots, wshm itself and a Reporter do not.
        on(&mut config).queue.required_approvals = 2;
        let server = MockServer::start().await;
        let gl = gitlab_with(&server, live_mr("abc", "success", false));
        let msg = merge_top(&config, &db, &gl, &scored()).await;
        assert!(msg.contains("1/2 approval"), "{msg}");
        assert!(puts(&server).is_empty());

        // No approval at all: a high score (AI priority label) does not
        // stand in for one.
        config.queue.required_approvals = 1;
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/merge_requests/5"),
            Reply::json(live_mr("abc", "success", false)),
        );
        server.on(
            "GET",
            &format!("{P}/merge_requests/5/approvals"),
            Reply::json(json!({"approved_by": [], "approvals_left": 0})),
        );
        let gl = GitLabProvider::with_rest(
            Rest::new("GitLab", Auth::Header("Authorization", "Bearer t".into())).unwrap(),
            &server.url(),
            "g/p",
            "<!-- wshm -->".into(),
        );
        let msg = merge_top(&config, &db, &gl, &scored()).await;
        assert!(msg.contains("0/1 approval"), "{msg}");
        assert!(puts(&server).is_empty());
    }

    #[tokio::test]
    async fn failed_merge_put_is_not_retried_and_reports_the_real_state() {
        let db = crate::db::Database::open_memory().unwrap();
        let mut config = Config::default();
        let config = on(&mut config);

        // 502 but GitLab merged it anyway: reported as merged.
        let server = MockServer::start().await;
        server.on(
            "PUT",
            &format!("{P}/merge_requests/5/merge"),
            Reply::json(json!({"message": "502"})).status(502),
        );
        let gl = gitlab_with(&server, live_mr("abc", "success", false));
        let sha = merge_gate(config, &db, &gl, &scored()).await.unwrap();
        assert_eq!(sha, "abc");
        // The re-read after the error sees it merged.
        let server2 = MockServer::start().await;
        server2.on(
            "PUT",
            &format!("{P}/merge_requests/5/merge"),
            Reply::json(json!({"message": "502"})).status(502),
        );
        server2.on(
            "GET",
            &format!("{P}/merge_requests/5"),
            Reply::json(json!({"iid": 5, "state": "merged"})),
        );
        let gl2 = GitLabProvider::with_rest(
            Rest::new("GitLab", Auth::Header("Authorization", "Bearer t".into())).unwrap(),
            &server2.url(),
            "g/p",
            "<!-- wshm -->".into(),
        );
        assert_eq!(
            gl2.merge_pr(5, Some(&sha), false).await.unwrap(),
            MergeOutcome::Merged
        );
        assert_eq!(puts(&server2).len(), 1);

        // 502 and still open: an error that says so, one PUT only.
        let msg = merge_top(config, &db, &gl, &scored()).await;
        assert!(
            msg.starts_with("Failed to merge PR #5") && msg.contains("not merged"),
            "{msg}"
        );
        assert_eq!(puts(&server).len(), 1);
    }

    #[tokio::test]
    async fn forge_without_merge_support_only_reports() {
        let db = crate::db::Database::open_memory().unwrap();
        let mut config = Config::default();
        let rec = crate::git_provider::recording::RecordingForge::default();
        let msg = merge_top(on(&mut config), &db, &rec, &scored()).await;
        assert!(msg.contains("not supported"), "{msg}");
    }
}
