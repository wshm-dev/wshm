//! Review steps of the `anthropic` flow: `context` (repository guidelines
//! and the code around the changes), the collect mode of `review`,
//! `security` (a dedicated security pass) and `verify` (voters confirm each
//! finding; only confirmed ones are posted).
//!
//! Every text that comes from the PR, the repository or a model is wrapped
//! as an untrusted block (`ai::untrusted`), every model output goes through
//! `ai::output::validate_review`, and nothing is posted for an item whose
//! content is flagged as a possible prompt injection.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use async_trait::async_trait;
use tracing::{info, warn};

use super::params::{self, Param};
use super::state::{added_lines, severity_of, ReviewContext, ReviewFinding, Severity};
use super::steps::{Step, StepOutput};
use super::template::StepDef;
use super::{ItemKind, RunContext};
use crate::ai::prompts::{security_review, verify_findings};
use crate::ai::schemas::{InlineComment, InlineReviewResult, ReviewStats};

const PR: &[ItemKind] = &[ItemKind::Pr];

/// Max size of one guideline file in the prompt.
const MAX_DOC_BYTES: usize = 8000;
/// Findings sent to the voters (the rest is not verified, so not posted).
const MAX_VERIFY: usize = 50;
/// Posted-finding fingerprints kept per PR.
const MAX_FINGERPRINTS: usize = 500;

/// `with` of `context`.
const CONTEXT_PARAMS: &[(&str, Param)] = &[
    ("files", Param::List),
    ("lines", Param::Int(0, 200)),
    ("max_bytes", Param::Int(1000, 200_000)),
];

/// `context`: load the repo guidelines (CLAUDE.md, REVIEW.md) and the code
/// around the changed lines, from the local checkout of the default branch
/// (never from the PR). Done even when there is nothing to load, so the
/// review steps that need it still run.
pub struct ContextStep;

#[async_trait]
impl Step for ContextStep {
    fn kind(&self) -> &'static str {
        "context"
    }

    fn items(&self) -> &'static [ItemKind] {
        PR
    }

    fn mutating(&self) -> bool {
        false
    }

    fn validate_params(
        &self,
        p: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        params::check(self.kind(), p, CONTEXT_PARAMS)?;
        for f in params::list_or(p, "files", &[]) {
            if crate::ai::output::sanitize_path(&f).is_none() {
                return Err(format!("context: invalid file path '{f}'"));
            }
        }
        Ok(())
    }

    async fn run(&self, ctx: &RunContext, def: &StepDef) -> anyhow::Result<StepOutput> {
        let Some(repo) = super::repo_files::default_checkout(&ctx.config) else {
            ctx.state().context = Some(ReviewContext::default());
            return Ok(StepOutput::done(
                "no local checkout of the default branch: no guidelines or code context loaded",
            ));
        };
        let files = params::list_or(&def.with, "files", &["CLAUDE.md", "REVIEW.md"]);
        let around = params::u64_or(&def.with, "lines", 20) as usize;
        let max_bytes = params::u64_or(&def.with, "max_bytes", 30_000) as usize;

        let mut c = ReviewContext::default();
        let mut loaded = Vec::new();
        for f in &files {
            let Some(rel) = crate::ai::output::sanitize_path(f) else {
                continue;
            };
            if let Some(text) = repo.read(&rel, MAX_DOC_BYTES) {
                let block = crate::ai::untrusted::wrap_context_whole(
                    "repository guidelines",
                    &format!("# {rel}\n\n{text}"),
                );
                if block.is_empty() {
                    info!(
                        "[{}] {rel} left out of the review context (suspicious)",
                        ctx.config.repo_slug()
                    );
                } else {
                    c.docs.push_str(&block);
                    loaded.push(rel);
                }
            }
        }

        let diff = ctx.pr_diff().await.map_err(anyhow::Error::msg)?;
        let mut snippets = 0;
        for (path, ranges) in old_hunks(&diff) {
            if c.code.len() >= max_bytes {
                break;
            }
            let Some(rel) = crate::ai::output::sanitize_path(&path) else {
                continue;
            };
            let Some(text) = repo.read(&rel, usize::MAX) else {
                continue;
            };
            let snippet = around_lines(&text, &ranges, around);
            if snippet.is_empty() {
                continue;
            }
            let left = max_bytes.saturating_sub(c.code.len());
            let end = crate::ai::prompts::truncate_utf8(&snippet, left);
            c.code.push_str(&crate::ai::untrusted::wrap_code(
                "repository file",
                &format!("# {rel}\n{}", &snippet[..end]),
            ));
            snippets += 1;
        }
        ctx.state().context = Some(c);
        let docs = if loaded.is_empty() {
            "no guidelines".to_string()
        } else {
            loaded.join(", ")
        };
        Ok(StepOutput::done(format!(
            "{docs}; code around the changes of {snippets} file(s) ({} at {})",
            repo.rev,
            &repo.commit[..repo.commit.len().min(7)]
        )))
    }
}

static RE_OLD_HUNK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+").unwrap());

/// Old-side line ranges (1-based, inclusive) of each file of a diff, by
/// old path. New files have none.
pub fn old_hunks(diff: &str) -> Vec<(String, Vec<(usize, usize)>)> {
    let mut out: Vec<(String, Vec<(usize, usize)>)> = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some((a, _)) = rest.split_once(" b/") {
                let a = a.trim();
                out.push((a.strip_prefix("a/").unwrap_or(a).to_string(), Vec::new()));
            }
        } else if let Some(c) = RE_OLD_HUNK.captures(line) {
            let start: usize = c[1].parse().unwrap_or(0);
            let len: usize = c.get(2).map_or(1, |m| m.as_str().parse().unwrap_or(1));
            if let (Some(last), true) = (out.last_mut(), start > 0 && len > 0) {
                last.1.push((start, start + len - 1));
            }
        }
    }
    out.retain(|(_, r)| !r.is_empty());
    out
}

/// The lines of `text` within `around` lines of `ranges`, numbered.
fn around_lines(text: &str, ranges: &[(usize, usize)], around: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut keep = vec![false; lines.len()];
    for &(a, b) in ranges {
        let from = a.saturating_sub(around + 1);
        let to = (b + around).min(lines.len());
        for k in keep.iter_mut().take(to).skip(from) {
            *k = true;
        }
    }
    let mut out = String::new();
    let mut last: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        if last.is_some_and(|j| j + 1 != i) {
            out.push_str("  ...\n");
        }
        out.push_str(&format!("{:>5} {l}\n", i + 1));
        last = Some(i);
    }
    out
}

fn context_prompt(ctx: &RunContext) -> String {
    ctx.state()
        .context
        .as_ref()
        .map(ReviewContext::prompt)
        .unwrap_or_default()
}

fn to_findings(comments: Vec<InlineComment>, diff: &str) -> Vec<(InlineComment, Severity)> {
    let added = added_lines(diff);
    comments
        .into_iter()
        .map(|c| {
            let s = severity_of(&c, &added);
            (c, s)
        })
        .collect()
}

/// `review` with `mode = "collect"`: the inline review, kept for the
/// `verify` step instead of being posted.
pub async fn collect_review(ctx: &RunContext, def: &StepDef) -> anyhow::Result<StepOutput> {
    let Some(n) = ctx.item.number else {
        return Ok(StepOutput::skipped("no PR number"));
    };
    let diff = ctx.pr_diff().await.map_err(anyhow::Error::msg)?;
    if diff.trim().is_empty() {
        return Ok(StepOutput::skipped("empty diff"));
    }
    // One call per file (or one for a small PR), reserved up front so the
    // step's and the run's AI budgets hold with steps running alongside.
    let needed = crate::pipelines::review::review_calls_needed(&diff);
    let granted = ctx.reserve_ai_calls(def, needed);
    if granted == 0 {
        return Ok(StepOutput::skipped("AI call budget reached"));
    }
    let ai = ctx.ai_for("review")?;
    let extra = context_prompt(ctx);
    let collected = crate::pipelines::review::collect(
        &ctx.config,
        ctx.db.as_ref(),
        &ai,
        n,
        &diff,
        &extra,
        granted,
    )
    .await?;
    let c = match collected {
        Ok(c) => c,
        Err(reason) => return Ok(StepOutput::skipped(reason)),
    };
    let items = to_findings(c.result.comments, &diff);
    let found = ctx.state().add_findings("review", items, &diff);
    let mut summary = format!("{found} finding(s) collected for verification");
    if c.files_left_out > 0 {
        summary.push_str(&format!(
            "; {} file(s) not reviewed (AI call budget)",
            c.files_left_out
        ));
    }
    Ok(StepOutput::done(summary).with_findings(found))
}

/// `review` in post mode: the findings of the review just posted, so a
/// later `status_check` sees them (they were posted as they are, so they
/// also stand as verified).
pub async fn record_posted_review(ctx: &RunContext, n: u64) {
    let Ok(Some(row)) = ctx.db.get_pr_review(n) else {
        return;
    };
    let Ok(diff) = ctx.pr_diff().await else {
        return;
    };
    let items = to_findings(row.result.comments, &diff);
    let mut st = ctx.state();
    st.add_findings("review", items, &diff);
    st.verified = Some(st.findings.clone());
}

const SECURITY_PARAMS: &[(&str, Param)] = &[("ai", Param::Bool), ("secrets", Param::Bool)];

/// `security`: a review dedicated to security. A pattern scan of the added
/// lines for committed secrets (no AI), then an AI pass on secrets,
/// injections and permissions. Findings go to `verify` like the review's.
pub struct SecurityStep;

#[async_trait]
impl Step for SecurityStep {
    fn kind(&self) -> &'static str {
        "security"
    }

    fn items(&self) -> &'static [ItemKind] {
        PR
    }

    fn mutating(&self) -> bool {
        false
    }

    fn validate_params(
        &self,
        p: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        params::check(self.kind(), p, SECURITY_PARAMS)
    }

    async fn run(&self, ctx: &RunContext, def: &StepDef) -> anyhow::Result<StepOutput> {
        let Some(n) = ctx.item.number else {
            return Ok(StepOutput::skipped("no PR number"));
        };
        let diff = ctx.pr_diff().await.map_err(anyhow::Error::msg)?;
        if diff.trim().is_empty() {
            return Ok(StepOutput::skipped("empty diff"));
        }
        let pr = ctx.db.get_pull(n)?;
        let (title, body) = pr
            .as_ref()
            .map(|p| (p.title.clone(), p.body.clone().unwrap_or_default()))
            .unwrap_or_default();

        // Injection detection over the PR text and the added lines.
        let guard_cfg = crate::ai::guard::load(&ctx.config, ctx.db.as_ref());
        let pr_text = format!("{title}\n\n{body}");
        let assessment = crate::ai::guard::assess_texts(
            &guard_cfg,
            &[
                ("pull request", pr_text.as_str()),
                ("pull request diff", diff.as_str()),
            ],
        );
        crate::ai::guard::record(ctx.db.as_ref(), "pull", n, "security", &assessment);

        let mut comments: Vec<InlineComment> = Vec::new();
        if params::bool_or(&def.with, "secrets", true) {
            comments.extend(secret_findings(&diff));
        }
        let secrets = comments.len();
        let mut note = String::new();
        if params::bool_or(&def.with, "ai", true) {
            if ctx.reserve_ai_calls(def, 1) == 0 {
                note = "; AI pass skipped (AI call budget reached)".into();
            } else {
                let ai = ctx.ai_for("security")?;
                let prompt =
                    security_review::build_prompt(&title, &body, &diff, &context_prompt(ctx));
                let mut r: InlineReviewResult =
                    ai.complete(security_review::SYSTEM, &prompt).await?;
                crate::ai::output::validate_review(&mut r, &diff);
                let seen: HashSet<(String, u64)> =
                    comments.iter().map(|c| (c.path.clone(), c.line)).collect();
                comments.extend(
                    r.comments
                        .into_iter()
                        .filter(|c| !seen.contains(&(c.path.clone(), c.line)))
                        .map(|mut c| {
                            c.category = "security".into();
                            c
                        }),
                );
            }
        }
        let items = to_findings(comments, &diff);
        let found = ctx.state().add_findings("security", items, &diff);
        Ok(StepOutput::done(format!(
            "{found} security finding(s), {secrets} from the secret patterns{note}"
        ))
        .with_findings(found))
    }
}

/// Added lines that look like a committed secret (one finding per line, 20
/// at most). The secret itself is never echoed.
fn secret_findings(diff: &str) -> Vec<InlineComment> {
    let mut out = Vec::new();
    for d in crate::ai::diff::lines(diff) {
        if d.kind != crate::ai::diff::LineKind::Added || out.len() >= 20 {
            continue;
        }
        let (Some(path), Some(line)) = (d.path, d.new_line) else {
            continue;
        };
        if crate::ai::output::has_secret(d.text) {
            out.push(InlineComment {
                path: path.to_string(),
                line,
                body: "This line looks like a committed secret (key, token or password). \
                       Remove it from the change, rotate it, and load it from the secret store."
                    .into(),
                severity: "error".into(),
                category: "security".into(),
                suggestion: None,
            });
        }
    }
    out
}

const VERIFY_PARAMS: &[(&str, Param)] = &[
    ("voters", Param::Int(1, 5)),
    ("min_votes", Param::Int(1, 5)),
    ("max_nits", Param::Int(0, 50)),
    ("post", Param::Bool),
];

/// `verify`: voters (one AI call each) vote on every finding of the run;
/// a finding is confirmed with `min_votes` votes (default: a majority).
/// Nits beyond `max_nits` are dropped. The confirmed findings are posted
/// as one review, tagged Important / Nit / Pre-existing, without repeating
/// findings already posted on the PR.
pub struct VerifyStep;

#[async_trait]
impl Step for VerifyStep {
    fn kind(&self) -> &'static str {
        "verify"
    }

    fn items(&self) -> &'static [ItemKind] {
        PR
    }

    fn validate_params(
        &self,
        p: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), String> {
        params::check(self.kind(), p, VERIFY_PARAMS)?;
        let voters = params::u64_or(p, "voters", 3);
        if params::u64_or(p, "min_votes", 1) > voters {
            return Err("verify: min_votes cannot exceed voters".into());
        }
        Ok(())
    }

    async fn run(&self, ctx: &RunContext, def: &StepDef) -> anyhow::Result<StepOutput> {
        let Some(n) = ctx.item.number else {
            return Ok(StepOutput::skipped("no PR number"));
        };
        let (reviewed, mut findings) = {
            let s = ctx.state();
            (s.reviewed, s.findings.clone())
        };
        if !reviewed {
            return Ok(StepOutput::skipped(
                "no findings to verify: no review or security step ran",
            ));
        }
        if findings.is_empty() {
            ctx.state().verified = Some(Vec::new());
            return Ok(StepOutput::done("no findings to verify"));
        }
        let unverified = findings.len().saturating_sub(MAX_VERIFY);
        findings.truncate(MAX_VERIFY);

        let wanted = params::u64_or(&def.with, "voters", 3) as u32;
        let min_votes = params::u64_or(&def.with, "min_votes", u64::from(wanted / 2 + 1)) as u32;
        // Never fewer voters than votes needed: then nothing is confirmed
        // and nothing posted, rather than one vote confirming everything.
        let budget = ctx.budget_for(def).map_or(wanted, |b| b.min(wanted));
        if budget < min_votes {
            let why = format!(
                "only {budget} voter(s) fit in the AI call budget, {min_votes} vote(s) needed: findings not verified, nothing posted"
            );
            ctx.state().verify_incomplete = Some(why.clone());
            return Ok(StepOutput::skipped(why));
        }
        let voters = ctx.reserve_ai_calls(def, wanted);
        if voters < min_votes {
            let why = format!(
                "only {voters} voter(s) fit in the AI call budget, {min_votes} vote(s) needed: findings not verified, nothing posted"
            );
            ctx.state().verify_incomplete = Some(why.clone());
            return Ok(StepOutput::skipped(why));
        }

        let diff = ctx.pr_diff().await.map_err(anyhow::Error::msg)?;
        let title = ctx.db.get_pull(n)?.map(|p| p.title).unwrap_or_default();
        let listing: Vec<serde_json::Value> = findings
            .iter()
            .map(|f| {
                serde_json::json!({
                    "id": f.id, "path": f.path, "line": f.line,
                    "severity": f.severity, "category": f.category, "finding": f.body,
                })
            })
            .collect();
        let listing = serde_json::to_string_pretty(&listing)?;
        let context = context_prompt(ctx);
        let ai = ctx.ai_for("verify")?;
        let calls = (0..voters as usize).map(|v| {
            let prompt = verify_findings::build_prompt(&title, &diff, &listing, &context, v);
            let ai = &ai;
            async move {
                ai.complete::<verify_findings::Votes>(verify_findings::SYSTEM, &prompt)
                    .await
            }
        });
        let results = futures::future::join_all(calls).await;

        let ids: HashSet<u32> = findings.iter().map(|f| f.id).collect();
        let mut yes: HashMap<u32, u32> = HashMap::new();
        let mut ok_voters = 0u32;
        let mut last_err = None;
        for r in results {
            match r {
                Ok(v) => {
                    ok_voters += 1;
                    let mut voted = HashSet::new();
                    for vote in v.votes {
                        // One vote per finding and voter; unknown ids ignored.
                        if ids.contains(&vote.id) && voted.insert(vote.id) && vote.real {
                            *yes.entry(vote.id).or_default() += 1;
                        }
                    }
                }
                Err(e) => {
                    warn!("verify voter failed on PR #{n}: {e:#}");
                    last_err = Some(e);
                }
            }
        }
        if ok_voters == 0 {
            ctx.state().verify_incomplete = Some("every verify voter failed".into());
            return Err(last_err
                .unwrap_or_else(|| anyhow::anyhow!("no voter answered"))
                .context("every verify voter failed"));
        }
        if ok_voters < min_votes {
            let why = format!(
                "only {ok_voters} of {voters} voter(s) answered, {min_votes} vote(s) needed: findings not verified, nothing posted"
            );
            ctx.state().verify_incomplete = Some(why.clone());
            return Ok(StepOutput::soft_failed(why));
        }
        let needed = min_votes.max(1);
        let max_nits = params::u64_or(&def.with, "max_nits", 3) as usize;
        let mut nits = 0;
        let confirmed: Vec<ReviewFinding> = findings
            .into_iter()
            .filter(|f| yes.get(&f.id).copied().unwrap_or(0) >= needed)
            .filter(|f| {
                if f.severity != Severity::Nit {
                    return true;
                }
                nits += 1;
                nits <= max_nits
            })
            .collect();
        let total = ids.len() + unverified;
        ctx.state().verified = Some(confirmed.clone());

        let post_note = post_confirmed(ctx, def, n, &confirmed).await;
        let mut summary = format!(
            "{} of {total} finding(s) confirmed ({ok_voters} voter(s), {needed} vote(s) needed)",
            confirmed.len()
        );
        if unverified > 0 {
            summary.push_str(&format!(", {unverified} not verified (over {MAX_VERIFY})"));
        }
        summary.push_str("; ");
        summary.push_str(&post_note);
        Ok(StepOutput::done(summary).with_findings(confirmed.len() as u32))
    }
}

fn fingerprints_key(n: u64) -> String {
    format!("pipeline_posted_findings:{n}")
}

/// Atomically change the stored fingerprints of PR `n` with `f` (which
/// returns the new list, or `None` to leave it). Compare-and-set, retried.
fn update_fingerprints<F>(ctx: &RunContext, n: u64, mut f: F) -> bool
where
    F: FnMut(&[String]) -> Option<Vec<String>>,
{
    let key = fingerprints_key(n);
    for _ in 0..5 {
        let raw = ctx.db.get_app_setting(&key).ok().flatten();
        let current: Vec<String> = raw
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        let Some(mut next) = f(&current) else {
            return true;
        };
        let start = next.len().saturating_sub(MAX_FINGERPRINTS);
        next.drain(..start);
        let Ok(j) = serde_json::to_string(&next) else {
            return false;
        };
        let ok = match &raw {
            None => ctx.db.set_app_setting_if_absent(&key, &j),
            Some(old) => ctx.db.compare_and_set_app_setting(&key, old, &j),
        };
        match ok {
            Ok(true) => return true,
            Ok(false) => continue,
            Err(e) => {
                warn!("Could not store the posted findings of PR #{n}: {e:#}");
                return false;
            }
        }
    }
    false
}

/// The confirmed findings not posted before (each fingerprint once, also
/// within this batch), claimed in the store. Empty when the store cannot
/// be updated (nothing is posted rather than risking a double post).
fn claim_fingerprints<'a>(
    ctx: &RunContext,
    n: u64,
    confirmed: &'a [ReviewFinding],
) -> Vec<&'a ReviewFinding> {
    let mut fresh = Vec::new();
    let claimed = update_fingerprints(ctx, n, |current| {
        fresh.clear();
        let mut seen: HashSet<String> = current.iter().cloned().collect();
        for f in confirmed {
            if seen.insert(f.fingerprint()) {
                fresh.push(f);
            }
        }
        if fresh.is_empty() {
            return None;
        }
        let mut next = current.to_vec();
        next.extend(fresh.iter().map(|f| f.fingerprint()));
        Some(next)
    });
    if claimed {
        fresh
    } else {
        Vec::new()
    }
}

fn release_fingerprints(ctx: &RunContext, n: u64, fresh: &[&ReviewFinding]) {
    let gone: HashSet<String> = fresh.iter().map(|f| f.fingerprint()).collect();
    update_fingerprints(ctx, n, |current| {
        Some(
            current
                .iter()
                .filter(|f| !gone.contains(*f))
                .cloned()
                .collect(),
        )
    });
}

/// Store the review (always) and post the confirmed findings not posted
/// before (apply mode, not flagged). Returns what happened, for the
/// summary.
async fn post_confirmed(
    ctx: &RunContext,
    def: &StepDef,
    n: u64,
    confirmed: &[ReviewFinding],
) -> String {
    let important = confirmed
        .iter()
        .filter(|f| f.severity == Severity::Important)
        .count();
    let summary = if confirmed.is_empty() {
        "No issues found.".to_string()
    } else {
        format!("{} issue(s), {important} important.", confirmed.len())
    };
    let mut stats = ReviewStats::default();
    let comments: Vec<InlineComment> = confirmed.iter().map(ReviewFinding::to_comment).collect();
    for c in &comments {
        match c.severity.as_str() {
            "error" => stats.errors += 1,
            "warning" => stats.warnings += 1,
            _ => stats.infos += 1,
        }
    }
    let result = InlineReviewResult {
        comments,
        summary: summary.clone(),
        stats,
    };

    let mut posted = false;
    let note = if !params::bool_or(&def.with, "post", true) {
        "not posted (post = false)".to_string()
    } else if !ctx.should_post_review() {
        "not posted (dry-run or review_post_comments off)".to_string()
    } else if ctx.mutations_blocked() {
        "not posted (flagged as possible prompt injection)".to_string()
    } else {
        // Claim the new fingerprints before posting (compare-and-set), so
        // two runs on the same PR (other pod) cannot post them twice; a
        // failed post gives them back.
        let fresh = claim_fingerprints(ctx, n, confirmed);
        if fresh.is_empty() {
            if confirmed.is_empty() {
                "nothing to post".to_string()
            } else {
                "nothing new to post (already posted)".to_string()
            }
        } else {
            let inline: Vec<(String, u64, String)> = fresh
                .iter()
                .map(|f| {
                    let body = format!(
                        "**{}** · {}",
                        f.severity.label(),
                        crate::pipelines::review::format_github_comment(&f.to_comment())
                    );
                    (
                        f.path.clone(),
                        f.line,
                        crate::pipelines::truncate(&body, 2000),
                    )
                })
                .collect();
            let body = format!(
                "{}## Automated Code Review\n\n{} {} new finding(s) confirmed by verification.\n\n{}",
                ctx.config.branding.header(),
                summary,
                fresh.len(),
                ctx.config.branding.footer("Reviewed"),
            );
            match ctx.gh.submit_review(n, &body, &inline).await {
                Ok(()) => {
                    posted = true;
                    format!("{} posted", fresh.len())
                }
                Err(e) => {
                    warn!("Failed to post the verified review on PR #{n}: {e:#}");
                    release_fingerprints(ctx, n, &fresh);
                    format!("posting failed: {e}")
                }
            }
        }
    };
    if let Err(e) = ctx.db.upsert_pr_review(&crate::db::reviews::PrReviewRow {
        pr_number: n,
        result,
        posted_to_github: posted,
        content_hash: None,
        reviewed_at: chrono::Utc::now().to_rfc3339(),
    }) {
        warn!("Failed to store the verified review of PR #{n}: {e:#}");
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_hunks_and_snippets() {
        let diff = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -3,2 +3,3 @@\n c\n+x\n d\ndiff --git a/new.rs b/new.rs\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+n\n";
        let h = old_hunks(diff);
        assert_eq!(h, [("src/a.rs".to_string(), vec![(3, 4)])]);
        let text = (1..=20).map(|i| format!("l{i}\n")).collect::<String>();
        let s = around_lines(&text, &h[0].1, 1);
        assert_eq!(s, "    2 l2\n    3 l3\n    4 l4\n    5 l5\n");
        let s = around_lines(&text, &[(2, 2), (10, 10)], 0);
        assert_eq!(s, "    2 l2\n  ...\n   10 l10\n");
    }

    #[test]
    fn secret_patterns_on_added_lines_only() {
        let diff = "diff --git a/c.env b/c.env\n--- a/c.env\n+++ b/c.env\n@@ -1,1 +1,3 @@\n-token = ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAA\n+token = ghp_BBBBBBBBBBBBBBBBBBBBBBBBBBBB\n+ok = 1\n+key = ghp_CCCCCCCCCCCCCCCCCCCCCCCCCCCC\n";
        let f = secret_findings(diff);
        assert_eq!(f.len(), 2, "two secrets in one file are two findings");
        assert_eq!(f[1].line, 3);
        assert_eq!((f[0].path.as_str(), f[0].line), ("c.env", 1));
        assert!(!f[0].body.contains("ghp_"), "the secret is never echoed");
    }
}
