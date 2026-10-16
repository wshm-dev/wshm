use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::ai::backend::AiBackend;
use crate::ai::prompts::pr_analyze;
use crate::ai::schemas::PrAnalysis;
use crate::cli::PrArgs;
use crate::config::Config;
use crate::db::backend::DatabaseBackend;
use crate::db::pulls::{PrAnalysisRow, PullRequest};
use crate::export::{EventKind, ExportEvent, ExportManager};
use crate::git_provider::Forge as GhClient;

/// What to do with the (slow) diagram step of `[pr.review_diagrams]`. The
/// analysis comment is always posted first, without waiting for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramMode {
    /// Run it right after posting, in this task (CLI). In a dry-run it only
    /// runs for an explicit preview.
    Inline,
    /// Return it as a [`DiagramJob`] for the caller to spawn (daemon).
    Deferred,
    /// Only reuse a cached diagram, never generate.
    Skip,
}

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    /// Print the comment instead of posting it (single-PR CLI dry-run).
    pub preview: bool,
    pub diagrams: DiagramMode,
}

/// A pending diagram step: generate the architecture section and, if it
/// differs from what was posted, edit the analysis comment in place.
#[derive(Debug, Clone)]
pub struct DiagramJob {
    pub pr: PullRequest,
    diff: String,
    analysis: PrAnalysis,
    posted_section: Option<String>,
    apply: bool,
    preview: bool,
}

/// Concurrent background diagram steps, all repos together. Separate from
/// the event processor's permits so a slow AI never stalls event handling.
static DIAGRAM_JOBS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// Max diagram jobs waiting or running, all repos together. Beyond it new
/// jobs are dropped (the next push of the PR schedules one again).
const MAX_PENDING_JOBS: usize = 32;

/// Latest job id per `repo#pr`: a newer job for the same PR supersedes the
/// queued one, which then exits without any AI call.
static PENDING_JOBS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
    std::sync::LazyLock::new(Default::default);
static JOB_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Register a job for `key`, superseding any older one. `None` when the
/// queue is full.
fn register_job(key: &str) -> Option<u64> {
    let mut m = PENDING_JOBS.lock().ok()?;
    if m.len() >= MAX_PENDING_JOBS && !m.contains_key(key) {
        return None;
    }
    let id = JOB_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    m.insert(key.to_string(), id);
    Some(id)
}

fn is_latest_job(key: &str, id: u64) -> bool {
    PENDING_JOBS
        .lock()
        .map(|m| m.get(key) == Some(&id))
        .unwrap_or(false)
}

fn release_job(key: &str, id: u64) {
    if let Ok(mut m) = PENDING_JOBS.lock() {
        if m.get(key) == Some(&id) {
            m.remove(key);
        }
    }
}

/// Spawn a diagram job in the background (daemon). One job per PR at a
/// time in the queue (the newest wins), at most [`MAX_PENDING_JOBS`] queued.
pub fn spawn_diagram_job(
    config: std::sync::Arc<Config>,
    db: std::sync::Arc<dyn DatabaseBackend>,
    gh: std::sync::Arc<GhClient>,
    job: DiagramJob,
) {
    let key = format!("{}#{}", config.repo_slug(), job.pr.number);
    let Some(id) = register_job(&key) else {
        tracing::warn!("PR #{}: diagram queue full, job dropped", job.pr.number);
        return;
    };
    tokio::spawn(async move {
        let Ok(_permit) = DIAGRAM_JOBS.acquire().await else {
            release_job(&key, id);
            return;
        };
        if is_latest_job(&key, id) {
            finish_diagram(&config, db.as_ref(), gh.as_ref(), job).await;
        } else {
            info!(
                "PR #{}: diagram job superseded by a newer one",
                job.pr.number
            );
        }
        release_job(&key, id);
    });
}

/// How a diagram job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagramOutcome {
    /// Nothing done, no AI call (dry-run, stale head, closed PR...).
    Skipped(&'static str),
    /// No diagram, or the same one as already posted.
    Unchanged,
    Posted,
    Previewed,
    Failed,
}

/// Is the job still worth running (or posting)? Checked before any AI call
/// and again before editing the comment.
fn job_skip_reason(db: &dyn DatabaseBackend, job: &DiagramJob) -> Option<&'static str> {
    if !job.apply && !job.preview {
        return Some("dry-run");
    }
    match db.get_pull(job.pr.number).ok().flatten() {
        None => Some("PR not in the cache"),
        Some(p) if p.state != "open" => Some("PR no longer open"),
        Some(p) if p.head_sha != job.pr.head_sha => Some("stale head"),
        Some(_) => None,
    }
}

/// Run a diagram job: bounded by `[pr.review_diagrams].timeout_secs`, never
/// fails. No AI call unless the PR is still open at the same head; edits
/// the comment only if that still holds after generation.
pub async fn finish_diagram(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    job: DiagramJob,
) -> DiagramOutcome {
    if let Some(why) = job_skip_reason(db, &job) {
        info!("PR #{}: diagram skipped ({why})", job.pr.number);
        return DiagramOutcome::Skipped(why);
    }
    let ai = match AiBackend::from_config(config, config.model_for("pr")) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(
                "PR #{}: diagram skipped, no AI backend: {e:#}",
                job.pr.number
            );
            return DiagramOutcome::Failed;
        }
    };
    let section =
        crate::diagrams::comment_section(config, &ai, db, gh, &job.pr, Some(&job.diff), job.apply)
            .await;
    if section.is_none() || section == job.posted_section {
        return DiagramOutcome::Unchanged;
    }
    let comment = format_analysis_comment(&job.analysis, config, section.as_deref());
    if job.apply {
        if let Some(why) = job_skip_reason(db, &job) {
            info!("PR #{}: diagram not posted ({why})", job.pr.number);
            return DiagramOutcome::Skipped(why);
        }
        match gh.comment_pr(job.pr.number, &comment).await {
            Ok(()) => {
                info!(
                    "PR #{}: architecture diagram added to the analysis comment",
                    job.pr.number
                );
                DiagramOutcome::Posted
            }
            Err(e) => {
                tracing::warn!("PR #{}: could not add the diagram: {e:#}", job.pr.number);
                DiagramOutcome::Failed
            }
        }
    } else {
        println!(
            "--- comment preview, updated with the diagram (dry-run, not posted) ---\n{comment}\n--- end of preview ---"
        );
        DiagramOutcome::Previewed
    }
}

#[derive(Serialize)]
struct PrAnalysisOutput {
    pr_number: u64,
    title: String,
    applied: bool,
    analysis: PrAnalysis,
}

/// CLI entry point: a single-PR dry-run previews the comment (and its
/// diagram); with `--apply` the diagram step runs right after posting.
pub async fn run(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    args: &PrArgs,
    json: bool,
    exporter: Option<&ExportManager>,
) -> Result<()> {
    let opts = RunOptions {
        preview: args.pr.is_some() && !args.apply && !json,
        diagrams: DiagramMode::Inline,
    };
    run_with(config, db, gh, args, json, exporter, opts).await?;
    Ok(())
}

/// Analyze PRs. Returns the deferred diagram jobs (`DiagramMode::Deferred`).
pub async fn run_with(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    args: &PrArgs,
    json: bool,
    exporter: Option<&ExportManager>,
    opts: RunOptions,
) -> Result<Vec<DiagramJob>> {
    let model = config.model_for("pr");
    let ai = AiBackend::from_config(config, model)?;

    // Infer the repo's grand domains once if none are configured yet, so the
    // user never has to guess them. Best-effort — never blocks analysis.
    crate::pipelines::discover_domains::ensure_discovered(config, db).await;

    let pulls = if let Some(number) = args.pr {
        match db.get_pull(number)? {
            Some(pr) => vec![pr],
            None => {
                if json {
                    println!("[]");
                } else {
                    println!("PR #{number} not found in cache. Run `wshm sync` first.");
                }
                return Ok(Vec::new());
            }
        }
    } else {
        db.get_pulls_needing_analysis()?
    };

    if pulls.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No PRs to analyze.");
        }
        return Ok(Vec::new());
    }

    // Polling mode has no `closed` webhook: drop diagrams of closed PRs here.
    if args.pr.is_none() && args.apply {
        crate::diagrams::sweep(config, db, gh).await;
    }

    let mut jobs: Vec<DiagramJob> = Vec::new();
    let mut results: Vec<PrAnalysisOutput> = Vec::with_capacity(pulls.len());

    for pr in &pulls {
        if config.prs_blacklist.contains(&pr.number) {
            info!("Skipping blacklisted PR #{}", pr.number);
            continue;
        }

        info!("Analyzing PR #{}: {}", pr.number, pr.title);
        match analyze_pr(config, &ai, db, gh, pr, args.apply, opts.preview, exporter).await {
            Ok((analysis, job)) => {
                // The diagram never runs in a dry-run unless explicitly
                // previewed: no AI spent on a comment nobody will see.
                if let Some(job) = job.filter(|j| j.apply || j.preview) {
                    match opts.diagrams {
                        DiagramMode::Inline => {
                            finish_diagram(config, db, gh, job).await;
                        }
                        DiagramMode::Deferred if job.apply => jobs.push(job),
                        _ => {}
                    }
                }
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
                // The AI provider is rate limited — every remaining PR in this
                // batch will fail identically, so stop instead of burning a
                // failed request per PR every cycle.
                if crate::pipelines::is_usage_limit_error(&e) {
                    tracing::warn!(
                        "Aborting PR analysis batch — AI usage limit reached ({} PRs left)",
                        pulls.len() - results.len()
                    );
                    break;
                }
            }
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }

    Ok(jobs)
}

#[allow(clippy::too_many_arguments)]
async fn analyze_pr(
    config: &Config,
    ai: &AiBackend,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    pr: &PullRequest,
    apply: bool,
    preview: bool,
    exporter: Option<&ExportManager>,
) -> Result<(PrAnalysis, Option<DiagramJob>)> {
    // Content hash cache: skip LLM call if content hasn't changed since last
    // analysis. The hash covers title + body + head_sha + labels, so a new
    // push (head_sha change) or an edit re-triggers analysis, but a no-op
    // event (e.g. assignee change) reuses the cached result.
    let content_hash = crate::db::schema::compute_pr_hash(
        &pr.title,
        pr.body.as_deref(),
        pr.head_sha.as_deref(),
        &pr.labels,
    );

    if let Ok(Some(existing)) = db.get_pr_analysis(pr.number) {
        if existing.content_hash.as_deref() == Some(content_hash.as_str()) {
            tracing::info!(
                "Skipping LLM for PR #{} — content unchanged (hash match)",
                pr.number
            );
            // Reconstruct PrAnalysis from the cached row. suggested_labels and
            // linked_issues were applied at first analysis and are not stored,
            // so they stay empty (we won't re-apply on a cache hit).
            let review_checklist = existing
                .review_notes
                .as_deref()
                .and_then(|n| serde_json::from_str(n).ok())
                .unwrap_or_default();
            return Ok((
                PrAnalysis {
                    summary: existing.summary,
                    risk_level: existing.risk_level,
                    pr_type: existing.pr_type,
                    linked_issues: Vec::new(),
                    review_checklist,
                    suggested_labels: Vec::new(),
                    domains: existing.domains,
                },
                None,
            ));
        }
    }

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

    // Related history (closed issues / merged PRs) from the full-text
    // index — see ai::related and `[ai.rag]`.
    user_prompt.push_str(&crate::ai::related::prompt_block(
        db,
        &config.repo_slug(),
        "pull",
        pr.number,
        &pr.title,
        pr.body.as_deref(),
        &config.ai.rag,
    ));

    // Inject custom label definitions if configured
    let labels_prompt = config.labels_prompt();
    if !labels_prompt.is_empty() {
        user_prompt.push_str(&labels_prompt);
    }

    // Inject the configured "grand domains" so the AI can tag the PR with them.
    // Source is the DB (stateless pods — see db::settings), not TOML config.
    let review_domains: Vec<crate::config::DomainDef> = db
        .get_app_setting(crate::db::settings::REVIEW_DOMAINS_KEY)
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let review_prompt = db
        .get_app_setting(crate::db::settings::REVIEW_PROMPT_KEY)
        .ok()
        .flatten();
    let domains_prompt = crate::config::domains_prompt(&review_domains, review_prompt.as_deref());
    if !domains_prompt.is_empty() {
        user_prompt.push_str(&domains_prompt);
    }

    // Inject configured Skills that apply to PR review (see config::skills_prompt).
    let skills = crate::config::load_skills(db);
    let skills_prompt = crate::config::skills_prompt(&skills, "pr_review");
    if !skills_prompt.is_empty() {
        user_prompt.push_str(&skills_prompt);
    }

    if !icm_context.is_empty() {
        user_prompt.push_str(&format!(
            "\n\n## Past PR analysis context (from memory)\n{icm_context}"
        ));
    }

    let system_prompt = config
        .pr
        .system_prompt
        .as_deref()
        .unwrap_or(pr_analyze::SYSTEM);

    let analysis: PrAnalysis = ai.complete(system_prompt, &user_prompt).await?;

    // Accumulate any newly-seen domains into the DB known set (best-effort),
    // so future reviews reuse them — the stateless-safe replacement for ICM
    // (which isn't installed on the pods).
    let new_domains: Vec<String> = analysis
        .domains
        .iter()
        .filter(|d| {
            !review_domains
                .iter()
                .any(|k| k.name.eq_ignore_ascii_case(d))
        })
        .cloned()
        .collect();
    if !new_domains.is_empty() {
        let mut merged = review_domains.clone();
        for n in new_domains {
            merged.push(crate::config::DomainDef {
                name: n,
                description: None,
                validated: false,
            });
        }
        if let Ok(j) = serde_json::to_string(&merged) {
            let _ = db.set_app_setting(crate::db::settings::REVIEW_DOMAINS_KEY, &j);
        }
    }

    // Account for the LLM call — advisory, never fail the pipeline.
    let _ = db.record_llm_invocation("pr_analysis", None);

    // Architecture diagram (`[pr.review_diagrams]`): only a cached one here
    // (DB read, no AI). Generation is a separate, bounded step that runs
    // after the comment is posted (see `DiagramJob`).
    let diagram = crate::diagrams::cached_section(config, db, gh, pr, diff.as_deref());

    // Store in DB
    let now = chrono::Utc::now().to_rfc3339();
    db.upsert_pr_analysis(&PrAnalysisRow {
        pr_number: pr.number,
        summary: analysis.summary.clone(),
        risk_level: analysis.risk_level.clone(),
        pr_type: analysis.pr_type.clone(),
        review_notes: Some(serde_json::to_string(&analysis.review_checklist)?),
        domains: analysis.domains.clone(),
        analyzed_at: now,
        content_hash: Some(content_hash),
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
        let mut labels = config.filter_labels(analysis.suggested_labels.clone());
        // Apply assigned "grand domains" as `domain:<name>` labels, but ONLY for
        // VALIDATED domains — proposed ones wait for human approval. GitHub
        // auto-creates missing labels; not run through filter_labels.
        let validated: std::collections::HashSet<String> = review_domains
            .iter()
            .filter(|d| d.validated)
            .map(|d| d.name.to_lowercase())
            .collect();
        labels.extend(
            analysis
                .domains
                .iter()
                .filter(|d| validated.contains(&d.to_lowercase()))
                .map(|d| format!("domain:{d}")),
        );
        if !labels.is_empty() {
            gh.label_pr(pr.number, &labels).await?;
        }

        // Auto-assign PR
        if config.assign.enabled {
            if let Some(assignee) = crate::config::AssignConfig::pick(&config.assign.prs) {
                info!("Auto-assigning PR #{} to {assignee}", pr.number);
                gh.add_pr_assignees(pr.number, &[assignee.to_string()])
                    .await?;
            }
        }

        let comment = format_analysis_comment(&analysis, config, diagram.as_deref());
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
    } else if preview {
        println!(
            "--- comment preview (dry-run, not posted) ---\n{}\n--- end of preview ---",
            format_analysis_comment(&analysis, config, diagram.as_deref())
        );
    }

    let job = diff.map(|diff| DiagramJob {
        pr: pr.clone(),
        diff,
        analysis: analysis.clone(),
        posted_section: diagram,
        apply,
        preview,
    });
    Ok((analysis, job))
}

/// `diagram`: the "Architecture" section, inserted before the checklist (or
/// at `{diagram}` in a custom `pr_template`).
fn format_analysis_comment(a: &PrAnalysis, config: &Config, diagram: Option<&str>) -> String {
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
            .replace(
                "{breaking_change}",
                check(a.review_checklist.breaking_change),
            )
            .replace("{docs_updated}", check(a.review_checklist.docs_updated))
            .replace("{linked_issues}", &linked_issues)
            .replace("{diagram}", diagram.unwrap_or(""))
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
         {}\
         ### Review Checklist\n\
         - [{}] Tests present\n\
         - [{}] Breaking change\n\
         - [{}] Docs updated\n",
        a.pr_type,
        a.risk_level,
        crate::pipelines::truncate(&a.summary, 500),
        diagram
            .map(|d| format!("{}\n\n", d.trim_end()))
            .unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn analysis() -> PrAnalysis {
        PrAnalysis {
            summary: "Adds X.".into(),
            risk_level: "low".into(),
            pr_type: "feature".into(),
            linked_issues: Vec::new(),
            review_checklist: Default::default(),
            suggested_labels: Vec::new(),
            domains: Vec::new(),
        }
    }

    #[test]
    fn diagram_section_goes_before_the_checklist() {
        let config = Config::default();
        let c = format_analysis_comment(&analysis(), &config, Some("### 🗺️ Architecture\n\nS\n"));
        let arch = c.find("### 🗺️ Architecture").unwrap();
        let summary = c.find("### Summary").unwrap();
        let checklist = c.find("### Review Checklist").unwrap();
        assert!(summary < arch && arch < checklist);
        assert!(c.contains("Adds X.\n\n### 🗺️ Architecture\n\nS\n\n### Review Checklist\n"));
        let without = format_analysis_comment(&analysis(), &config, None);
        assert!(without.contains("Adds X.\n\n### Review Checklist\n"));
    }

    fn job(apply: bool, preview: bool) -> DiagramJob {
        let pr: PullRequest = serde_json::from_value(serde_json::json!({
            "number": 5, "title": "t", "body": null, "state": "open", "labels": [],
            "author": null, "head_sha": "old", "base_sha": null, "head_ref": null,
            "base_ref": null, "mergeable": null, "ci_status": null,
            "created_at": "2026-10-01T00:00:00Z", "updated_at": "2026-10-01T00:00:00Z"
        }))
        .unwrap();
        DiagramJob {
            pr,
            diff: "diff --git a/x b/x\n+1\n".into(),
            analysis: analysis(),
            posted_section: None,
            apply,
            preview,
        }
    }

    async fn outcome(db_head: &str, db_state: &str, j: DiagramJob) -> DiagramOutcome {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let db = crate::db::Database::open_memory().unwrap();
        let mut cached = j.pr.clone();
        cached.head_sha = Some(db_head.into());
        cached.state = db_state.into();
        db.upsert_pull(&cached).unwrap();
        let config = Config::default();
        let gh = crate::git_provider::github::GitHubProvider::new(&config).unwrap();
        finish_diagram(&config, &db, &gh, j).await
    }

    #[tokio::test]
    async fn stale_closed_or_dry_run_jobs_make_no_ai_call() {
        assert_eq!(
            outcome("new", "open", job(true, false)).await,
            DiagramOutcome::Skipped("stale head")
        );
        assert_eq!(
            outcome("old", "closed", job(true, false)).await,
            DiagramOutcome::Skipped("PR no longer open")
        );
        assert_eq!(
            outcome("old", "open", job(false, false)).await,
            DiagramOutcome::Skipped("dry-run")
        );
    }

    #[test]
    fn newer_job_supersedes_the_queued_one() {
        let key = "test/repo#supersede";
        let first = register_job(key).unwrap();
        let second = register_job(key).unwrap();
        assert!(!is_latest_job(key, first));
        assert!(is_latest_job(key, second));
        release_job(key, first); // no effect on the newer one
        assert!(is_latest_job(key, second));
        release_job(key, second);
        assert!(!is_latest_job(key, second));
    }

    #[test]
    fn custom_template_places_the_diagram_at_its_placeholder() {
        let mut config = Config::default();
        config.branding.pr_template = Some("{summary}|{diagram}".into());
        assert_eq!(
            format_analysis_comment(&analysis(), &config, Some("D")),
            "Adds X.|D"
        );
        assert_eq!(
            format_analysis_comment(&analysis(), &config, None),
            "Adds X.|"
        );
    }
}
