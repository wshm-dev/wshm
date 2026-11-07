//! Nightly review (`[pipeline_nightly]`): existing code of the default
//! branch, outside any PR, a few files per night. Off by default.
//!
//! Started by the scheduler once per UTC day after `hour_utc`, by one pod
//! only (claimed in the shared DB). The files come from `git ls-files` of
//! the local checkout, filtered by `paths` / `exclude`; a cursor in the DB
//! makes each night go on where the previous one stopped. Each file is one
//! AI call, its content wrapped as an untrusted block and the model output
//! validated against the file's lines. The run is recorded like the other
//! runs (item kind `repo`, number 0, template `nightly`, one step); in
//! apply mode with `create_issue`, one issue lists the findings.

use std::path::Path;

use tracing::{info, warn};

use super::template::NightlyConfig;
use crate::ai::backend::AiBackend;
use crate::ai::prompts::nightly_review;
use crate::ai::schemas::{InlineComment, InlineReviewResult};
use crate::config::Config;
use crate::db::backend::DatabaseBackend;
use crate::db::runs::{NewPipelineRun, PipelineRunFinish, PipelineStepRunRow, PlannedStep};
use crate::git_provider::Forge;

const DAY_KEY: &str = "pipeline_nightly_day";
const CURSOR_KEY: &str = "pipeline_nightly_cursor";
/// Item kind of the recorded runs.
pub const ITEM_KIND: &str = "repo";
const MAX_FILE_BYTES: u64 = 60_000;
const MAX_LINES: u64 = 2000;

/// Claim today's nightly review for this repo: true once per UTC day, on
/// one pod, from `hour_utc` on.
pub fn claim(
    db: &dyn DatabaseBackend,
    cfg: &NightlyConfig,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    use chrono::Timelike;
    if !cfg.enabled || now.hour() < cfg.hour_utc {
        return false;
    }
    let today = now.format("%Y-%m-%d").to_string();
    match db.get_app_setting(DAY_KEY) {
        Ok(None) => db
            .set_app_setting_if_absent(DAY_KEY, &today)
            .unwrap_or(false),
        Ok(Some(prev)) if prev != today => db
            .compare_and_set_app_setting(DAY_KEY, &prev, &today)
            .unwrap_or(false),
        _ => false,
    }
}

/// Tracked files of the checkout matching the config, sorted.
pub fn candidate_files(root: &Path, cfg: &NightlyConfig) -> anyhow::Result<Vec<String>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let mut files: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .filter(|p| super::params::matches_any(&cfg.paths, p))
        .filter(|p| !super::params::matches_any(&cfg.exclude, p))
        .filter(|p| !crate::pipelines::review::should_skip_file(p))
        .filter_map(crate::ai::output::sanitize_path)
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// The next `max` files after the stored cursor (wrapping around), and the
/// new cursor.
fn next_batch(files: &[String], cursor: Option<&str>, max: usize) -> (Vec<String>, Option<String>) {
    if files.is_empty() {
        return (Vec::new(), None);
    }
    let start = cursor
        .map(|c| files.partition_point(|f| f.as_str() <= c))
        .unwrap_or(0);
    let batch: Vec<String> = (0..max.min(files.len()))
        .map(|i| files[(start + i) % files.len()].clone())
        .collect();
    let last = batch.last().cloned();
    (batch, last)
}

/// Comments of the model that point at a real line of the file, sanitized.
fn validate(r: &mut InlineReviewResult, path: &str, line_count: u64) {
    r.comments.retain(|c| c.line >= 1 && c.line <= line_count);
    for c in &mut r.comments {
        c.path = path.to_string();
        c.body = crate::ai::output::sanitize_posted_text(&c.body, 1000);
        c.suggestion = None;
    }
    r.comments.retain(|c| !c.body.is_empty());
    r.comments.truncate(5);
}

/// What a nightly review did.
#[derive(Debug, Default)]
pub struct NightlyReport {
    pub files: Vec<String>,
    pub findings: Vec<InlineComment>,
    /// Files left out because they look like a prompt injection attempt.
    pub flagged: Vec<String>,
    pub failed: u32,
    pub issue: Option<u64>,
}

/// Review the next batch of files with `ai`. Records the run.
pub async fn run_with(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &Forge,
    ai: &AiBackend,
    root: &Path,
    cfg: &NightlyConfig,
    apply: bool,
) -> anyhow::Result<NightlyReport> {
    let started = chrono::Utc::now();
    let t0 = std::time::Instant::now();
    let files = candidate_files(root, cfg)?;
    let cursor = db.get_app_setting(CURSOR_KEY).ok().flatten();
    let (batch, new_cursor) = next_batch(&files, cursor.as_deref(), cfg.max_files as usize);
    let guard_cfg = crate::ai::guard::load(config, db);

    let mut report = NightlyReport::default();
    let (_, calls) = super::count_ai_calls(async {
        for path in &batch {
            let full = root.join(path);
            let Ok(canon) = full.canonicalize() else {
                continue;
            };
            if !canon.starts_with(root) {
                continue;
            }
            match std::fs::metadata(&canon) {
                Ok(m) if m.is_file() && m.len() <= MAX_FILE_BYTES => {}
                _ => continue,
            }
            let Ok(text) = std::fs::read_to_string(&canon) else {
                continue;
            };
            let lines = text.lines().count() as u64;
            if lines == 0 || lines > MAX_LINES {
                continue;
            }
            report.files.push(path.clone());
            let a =
                crate::ai::guard::assess_texts(&guard_cfg, &[("repository file", text.as_str())]);
            if a.suspicious {
                report.flagged.push(path.clone());
                continue;
            }
            let prompt = nightly_review::build_prompt(path, &text);
            match ai
                .complete::<InlineReviewResult>(nightly_review::SYSTEM, &prompt)
                .await
            {
                Ok(mut r) => {
                    validate(&mut r, path, lines);
                    report.findings.extend(r.comments);
                }
                Err(e) => {
                    warn!(
                        "[{}] nightly review of {path} failed: {e:#}",
                        config.repo_slug()
                    );
                    report.failed += 1;
                }
            }
        }
    })
    .await;
    if let Some(c) = &new_cursor {
        if let Err(e) = db.set_app_setting(CURSOR_KEY, c) {
            warn!("Could not store the nightly review cursor: {e:#}");
        }
    }

    if apply && cfg.create_issue && !report.findings.is_empty() {
        let mut body = format!(
            "{}## Nightly review\n\n{} finding(s) in {} file(s) of the default branch.\n\n",
            config.branding.header(),
            report.findings.len(),
            report.files.len()
        );
        for c in &report.findings {
            body.push_str(&format!(
                "- `{}:{}` ({}, {}): {}\n",
                c.path, c.line, c.severity, c.category, c.body
            ));
        }
        body.push_str(&format!("\n{}", config.branding.footer("Reviewed")));
        let title = format!(
            "Nightly review: {} finding(s) in {} file(s)",
            report.findings.len(),
            report.files.len()
        );
        match gh.create_issue(&title, &body, &[]).await {
            Ok(n) => report.issue = Some(n),
            Err(e) => warn!(
                "[{}] could not open the nightly review issue: {e:#}",
                config.repo_slug()
            ),
        }
    }

    record(db, &report, started, t0.elapsed().as_millis() as u64, calls);
    Ok(report)
}

fn record(
    db: &dyn DatabaseBackend,
    r: &NightlyReport,
    started: chrono::DateTime<chrono::Utc>,
    duration_ms: u64,
    ai_calls: u32,
) {
    let plan = vec![PlannedStep {
        id: "nightly".into(),
        uses: "nightly".into(),
        needs: Vec::new(),
    }];
    let run = NewPipelineRun {
        item_kind: ITEM_KIND.into(),
        item_number: 0,
        template: "nightly".into(),
        trigger: "scheduler.nightly".into(),
        event_id: None,
        head_sha: None,
        plan_json: serde_json::to_string(&plan).unwrap_or_else(|_| "[]".into()),
        started_at: started.to_rfc3339(),
    };
    let id = match db.start_pipeline_run(&run) {
        Ok(Some(id)) => id,
        Ok(None) => return,
        Err(e) => {
            warn!("Could not record the nightly review run: {e:#}");
            return;
        }
    };
    let finished = chrono::Utc::now().to_rfc3339();
    let mut summary = format!(
        "{} finding(s) in {} file(s)",
        r.findings.len(),
        r.files.len()
    );
    if !r.flagged.is_empty() {
        summary.push_str(&format!(
            ", {} file(s) left out (suspicious)",
            r.flagged.len()
        ));
    }
    if r.failed > 0 {
        summary.push_str(&format!(", {} failed", r.failed));
    }
    if let Some(n) = r.issue {
        summary.push_str(&format!(", issue #{n} opened"));
    }
    let (status, reason) = if r.files.is_empty() {
        ("skipped", Some("no file to review".to_string()))
    } else if r.failed as usize == r.files.len() - r.flagged.len() && r.failed > 0 {
        ("failed", Some("every AI call failed".to_string()))
    } else {
        ("done", None)
    };
    let step = PipelineStepRunRow {
        run_id: id,
        seq: 0,
        step_id: "nightly".into(),
        uses: "nightly".into(),
        status: status.into(),
        reason,
        summary: Some(summary),
        findings: r.findings.len() as u32,
        ai_calls,
        started_at: started.to_rfc3339(),
        finished_at: finished.clone(),
        duration_ms,
    };
    if let Err(e) = db.record_pipeline_step(&step) {
        warn!("Could not record the nightly review step: {e:#}");
    }
    let fin = PipelineRunFinish {
        id,
        status: if status == "failed" { "failed" } else { "done" }.into(),
        error: None,
        head_sha: None,
        ai_calls,
        finished_at: finished,
        duration_ms,
    };
    if let Err(e) = db.finish_pipeline_run(&fin) {
        warn!("Could not finish the nightly review run: {e:#}");
    }
}

/// Scheduler entry: run the nightly review of the repo when it is due.
pub async fn maybe_run(config: &Config, db: &dyn DatabaseBackend, gh: &Forge, apply: bool) {
    let cfg = super::template::load(config, db).pipeline_nightly;
    if !claim(db, &cfg, chrono::Utc::now()) {
        return;
    }
    let slug = config.repo_slug();
    let Some(root) = config
        .wshm_dir
        .parent()
        .map(|p| {
            if p.as_os_str().is_empty() {
                Path::new(".")
            } else {
                p
            }
        })
        .and_then(|p| p.canonicalize().ok())
    else {
        warn!("[{slug}] nightly review: no local checkout");
        return;
    };
    let ai = match AiBackend::from_config(config, &config.ai.model) {
        Ok(a) => a,
        Err(e) => {
            warn!("[{slug}] nightly review: no AI backend: {e:#}");
            return;
        }
    };
    match run_with(config, db, gh, &ai, &root, &cfg, apply).await {
        Ok(r) => info!(
            "[{slug}] nightly review: {} finding(s) in {} file(s)",
            r.findings.len(),
            r.files.len()
        ),
        Err(e) => warn!("[{slug}] nightly review failed: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn claim_once_per_day_after_the_hour() {
        let db = crate::db::Database::open_memory().unwrap();
        let mut cfg = NightlyConfig::default();
        let at = |h| chrono::Utc.with_ymd_and_hms(2026, 11, 7, h, 0, 0).unwrap();
        assert!(!claim(&db, &cfg, at(5)), "off by default");
        cfg.enabled = true;
        assert!(!claim(&db, &cfg, at(2)), "before hour_utc");
        assert!(claim(&db, &cfg, at(3)));
        assert!(!claim(&db, &cfg, at(4)), "once per day");
        let next = chrono::Utc.with_ymd_and_hms(2026, 11, 8, 3, 0, 0).unwrap();
        assert!(claim(&db, &cfg, next));
    }

    #[test]
    fn batches_follow_the_cursor() {
        let f: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            next_batch(&f, None, 2),
            (vec!["a".into(), "b".into()], Some("b".into()))
        );
        assert_eq!(
            next_batch(&f, Some("b"), 2),
            (vec!["c".into(), "a".into()], Some("a".into()))
        );
        // A cursor on a file that no longer exists goes on after it.
        assert_eq!(next_batch(&f, Some("bb"), 1).0, vec!["c".to_string()]);
        assert_eq!(next_batch(&[], None, 2).0, Vec::<String>::new());
    }

    #[tokio::test]
    async fn reviews_files_and_records_the_run() {
        let dir = std::env::temp_dir().join(format!("wshm-nightly-{:x}", rand::random::<u64>()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let root = dir.canonicalize().unwrap();
        std::fs::write(root.join("src/a.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "x\n").unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&["add", "."]);

        let db = crate::db::Database::open_memory().unwrap();
        let gh = crate::git_provider::recording::RecordingForge::default();
        let ai = AiBackend::Mock(
            r#"{"comments":[{"path":"x","line":2,"body":"b never returns @someone","severity":"error","category":"bug"},{"path":"x","line":99,"body":"out of range"}],"summary":"1 issue"}"#
                .into(),
        );
        let cfg = NightlyConfig {
            enabled: true,
            create_issue: true,
            ..Default::default()
        };
        let config = Config::default();
        let r = run_with(&config, &db, &gh, &ai, &root, &cfg, true)
            .await
            .unwrap();
        assert_eq!(r.files, ["src/a.rs"], "README.md is outside paths");
        assert_eq!(r.findings.len(), 1, "line 99 is not in the file");
        assert_eq!(r.findings[0].path, "src/a.rs");
        assert!(
            !r.findings[0].body.contains("@someone"),
            "mentions neutralized"
        );
        assert_eq!(r.issue, Some(1000));
        assert_eq!(gh.any_of(&["create_issue"]).len(), 1);

        let runs = db
            .list_pipeline_runs(&crate::db::runs::PipelineRunFilter::default(), 10)
            .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            (runs[0].item_kind.as_str(), runs[0].template.as_str()),
            ("repo", "nightly")
        );
        assert_eq!(runs[0].steps[0].findings, 1);
        assert_eq!(runs[0].steps[0].ai_calls, 1);
        assert_eq!(runs[0].ai_calls, 1);

        // Dry-run: no issue.
        let gh = crate::git_provider::recording::RecordingForge::default();
        let r = run_with(&config, &db, &gh, &ai, &root, &cfg, false)
            .await
            .unwrap();
        assert!(r.issue.is_none() && gh.calls().is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}
