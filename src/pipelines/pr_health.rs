use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;

use super::truncate;
use crate::cli::HealthArgs;
use crate::config::{IssueScoringConfig, PrScoringConfig};
use crate::db::backend::DatabaseBackend;
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;
use crate::db::triage::TriageResultRow;

/// A group of duplicate PRs addressing the same topic
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroup {
    /// The "best" PR in the group (highest merge queue score)
    pub best: u64,
    /// All PR numbers in the group (including best)
    pub members: Vec<DuplicateMember>,
    /// Why they are considered duplicates
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateMember {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub score: i32,
    pub mergeable: Option<bool>,
    pub updated_at: String,
}

/// A stale/zombie PR with no recent activity
#[derive(Debug, Clone, Serialize)]
pub struct StalePr {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub days_stale: i64,
    pub mergeable: Option<bool>,
    pub has_conflicts: bool,
}

/// A PR that is too large and should be split
#[derive(Debug, Clone, Serialize)]
pub struct OversizedPr {
    pub number: u64,
    pub title: String,
    pub additions: usize,
    pub deletions: usize,
    pub files_changed: usize,
}

#[derive(Serialize)]
pub struct HealthReport {
    pub duplicates: Vec<DuplicateGroup>,
    pub stale: Vec<StalePr>,
    pub oversized: Vec<OversizedPr>,
}

pub fn run(db: &dyn DatabaseBackend, args: &HealthArgs, json: bool) -> Result<()> {
    let pulls = db.get_open_pulls()?;

    if pulls.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&HealthReport {
                    duplicates: Vec::new(),
                    stale: Vec::new(),
                    oversized: Vec::new(),
                })?
            );
        } else {
            println!("No open PRs.");
        }
        return Ok(());
    }

    let report = analyze_health(&pulls, args.stale_days);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    // Print duplicates
    if report.duplicates.is_empty() {
        println!("No duplicate PRs detected.");
    } else {
        println!(
            "Duplicate PR Groups ({} groups):\n",
            report.duplicates.len()
        );
        for (i, group) in report.duplicates.iter().enumerate() {
            println!(
                "  Group {} — {} (best: #{})",
                i + 1,
                group.reason,
                group.best
            );
            for m in &group.members {
                let marker = if m.number == group.best { "★" } else { " " };
                let mergeable_str = match m.mergeable {
                    Some(true) => "✓",
                    Some(false) => "✗",
                    None => "?",
                };
                println!(
                    "    {} #{:<5} score:{:<4} merge:{} @{:<20} {}",
                    marker,
                    m.number,
                    m.score,
                    mergeable_str,
                    m.author,
                    truncate(&m.title, 50),
                );
            }
            println!();
        }
    }

    // Print stale
    println!();
    if report.stale.is_empty() {
        println!("No stale PRs (threshold: {} days).", args.stale_days);
    } else {
        println!(
            "Stale/Zombie PRs ({}, threshold: {} days):\n",
            report.stale.len(),
            args.stale_days
        );
        println!(
            "  {:<6} {:<8} {:<10} {:<20} Title",
            "#", "Days", "Conflicts", "Author"
        );
        println!("  {}", "-".repeat(80));
        for s in &report.stale {
            let conflict_str = if s.has_conflicts { "conflict" } else { "ok" };
            println!(
                "  #{:<5} {:<8} {:<10} {:<20} {}",
                s.number,
                s.days_stale,
                conflict_str,
                s.author,
                truncate(&s.title, 40),
            );
        }
    }

    Ok(())
}

pub fn analyze_health(pulls: &[PullRequest], stale_days: i64) -> HealthReport {
    let duplicates = detect_duplicates(pulls);
    let stale = detect_stale(pulls, stale_days);
    HealthReport {
        duplicates,
        stale,
        oversized: Vec::new(),
    }
}

/// Detect duplicate PRs using Jaccard word similarity on titles + same linked issues
fn detect_duplicates(pulls: &[PullRequest]) -> Vec<DuplicateGroup> {
    let n = pulls.len();
    if n < 2 {
        return Vec::new();
    }

    // Precompute word sets for titles
    let word_sets: Vec<std::collections::HashSet<String>> =
        pulls.iter().map(|pr| tokenize(&pr.title)).collect();

    // Precompute linked issues for each PR
    let linked_issues: Vec<std::collections::HashSet<u64>> = pulls
        .iter()
        .map(|pr| extract_linked_issues(pr.body.as_deref().unwrap_or("")))
        .collect();

    // Build adjacency: which PRs are duplicates of each other
    // Use Union-Find for grouping
    let mut parent: Vec<usize> = (0..n).collect();

    for i in 0..n {
        for j in (i + 1)..n {
            let title_sim = jaccard_similarity(&word_sets[i], &word_sets[j]);
            let shared_issues = !linked_issues[i].is_empty()
                && !linked_issues[j].is_empty()
                && !linked_issues[i].is_disjoint(&linked_issues[j]);

            // Duplicate if: high title similarity OR same linked issue
            if title_sim >= 0.5 || shared_issues {
                union(&mut parent, i, j);
            }
        }
    }

    // Group by root
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }

    // Only keep groups with 2+ members
    let mut result: Vec<DuplicateGroup> = Vec::new();
    for indices in groups.values() {
        if indices.len() < 2 {
            continue;
        }

        let mut members: Vec<DuplicateMember> = indices
            .iter()
            .map(|&idx| {
                let pr = &pulls[idx];
                DuplicateMember {
                    number: pr.number,
                    title: pr.title.clone(),
                    author: pr.author.clone().unwrap_or_else(|| "unknown".into()),
                    score: score_pr(pr).0,
                    mergeable: pr.mergeable,
                    updated_at: pr.updated_at.clone(),
                }
            })
            .collect();

        // Sort by score descending
        members.sort_by_key(|b| std::cmp::Reverse(b.score));
        let best = members[0].number;

        // Determine reason
        let reason = determine_duplicate_reason(pulls, indices, &word_sets, &linked_issues);

        result.push(DuplicateGroup {
            best,
            members,
            reason,
        });
    }

    // Sort groups by size descending
    result.sort_by_key(|b| std::cmp::Reverse(b.members.len()));
    result
}

/// Detect stale/zombie PRs
fn detect_stale(pulls: &[PullRequest], threshold_days: i64) -> Vec<StalePr> {
    let now = chrono::Utc::now();
    let mut stale: Vec<StalePr> = pulls
        .iter()
        .filter_map(|pr| {
            let updated = pr
                .updated_at
                .parse::<chrono::DateTime<chrono::Utc>>()
                .ok()?;
            let days = now.signed_duration_since(updated).num_days();
            if days >= threshold_days {
                Some(StalePr {
                    number: pr.number,
                    title: pr.title.clone(),
                    author: pr.author.clone().unwrap_or_else(|| "unknown".into()),
                    days_stale: days,
                    mergeable: pr.mergeable,
                    has_conflicts: pr.mergeable == Some(false),
                })
            } else {
                None
            }
        })
        .collect();

    stale.sort_by_key(|b| std::cmp::Reverse(b.days_stale));
    stale
}

// --- Utility functions ---

fn tokenize(s: &str) -> std::collections::HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2) // skip short words like "a", "to", "in"
        .map(String::from)
        .collect()
}

fn jaccard_similarity(
    a: &std::collections::HashSet<String>,
    b: &std::collections::HashSet<String>,
) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn extract_linked_issues(body: &str) -> std::collections::HashSet<u64> {
    super::extract_linked_issue_numbers(body)
}

/// Compute a merge-readiness score for a PR, with optional breakdown strings.
/// Backward-compatible wrapper using the in-code default weights.
///
/// New callers that already hold a `Config` should prefer
/// [`score_pr_with`] so user-tuned `[scoring.pr]` weights take effect.
pub fn score_pr(pr: &PullRequest) -> (i32, Vec<String>) {
    score_pr_with(pr, &PrScoringConfig::default())
}

/// Rank an open PR for the merge queue.
///
/// Higher score → more ready to merge. The breakdown lists each signal
/// applied so the UI can explain *why* a PR ranks where it does.
///
/// Signals (defaults; override per-repo via `[scoring.pr]`):
/// - CI: +25 green, -30 failed, -5 pending
/// - mergeable: +2 / conflict: -25
/// - linked issue ("fixes #N"): +5
/// - labels: priority:critical/high/medium boosts; bug +5;
///   blocked/wip/do-not-merge sinks below the merge threshold
/// - age: +1/day capped at 10, then -15 once a PR crosses
///   `age_decay_after_days` (rotting PRs penalised, not rewarded)
/// - momentum: +5 if updated within 24h, -10 if dormant >30d
pub fn score_pr_with(pr: &PullRequest, w: &PrScoringConfig) -> (i32, Vec<String>) {
    let mut score = 0i32;
    let mut breakdown = Vec::new();

    // ─── CI signal ─────────────────────────────────────────────────
    match pr.ci_status.as_deref() {
        Some("success") => {
            score += w.ci_green;
            breakdown.push(format!("ci_green:{:+}", w.ci_green));
        }
        Some("failure") | Some("error") => {
            score += w.ci_failure;
            breakdown.push(format!("ci_failed:{:+}", w.ci_failure));
        }
        Some("pending") => {
            score += w.ci_pending;
            breakdown.push(format!("ci_pending:{:+}", w.ci_pending));
        }
        _ => {} // unknown / null → neutral
    }

    // ─── Mergeability ──────────────────────────────────────────────
    match pr.mergeable {
        Some(false) => {
            score += w.conflict;
            breakdown.push(format!("conflict:{:+}", w.conflict));
        }
        Some(true) => {
            score += w.mergeable;
            breakdown.push(format!("mergeable:{:+}", w.mergeable));
        }
        None => {} // unknown — GitHub still computing
    }

    // ─── Label-driven priority + blockers ──────────────────────────
    for label in &pr.labels {
        let l = label.to_ascii_lowercase();
        if l == "priority:critical" {
            score += w.label_priority_critical;
            breakdown.push(format!("critical:{:+}", w.label_priority_critical));
        } else if l == "priority:high" {
            score += w.label_priority_high;
            breakdown.push(format!("high:{:+}", w.label_priority_high));
        } else if l == "priority:medium" {
            score += w.label_priority_medium;
            breakdown.push(format!("medium:{:+}", w.label_priority_medium));
        } else if l == "bug" {
            score += w.label_bug;
            breakdown.push(format!("bug:{:+}", w.label_bug));
        } else if matches!(l.as_str(), "blocked" | "wip" | "do-not-merge" | "draft") {
            score += w.label_blocked;
            breakdown.push(format!("{l}:{:+}", w.label_blocked));
        }
    }

    // ─── Linked issue ──────────────────────────────────────────────
    if let Some(ref body) = pr.body {
        let lower = body.to_ascii_lowercase();
        if lower.contains("fixes #") || lower.contains("closes #") || lower.contains("resolves #") {
            score += w.linked_issue;
            breakdown.push(format!("linked:{:+}", w.linked_issue));
        }
    }

    // ─── Age (peak then decay) ─────────────────────────────────────
    if let Ok(created) = pr.created_at.parse::<chrono::DateTime<chrono::Utc>>() {
        let days = chrono::Utc::now().signed_duration_since(created).num_days();
        if days > w.age_decay_after_days {
            score += w.age_decay_penalty;
            breakdown.push(format!("rotting_{}d:{:+}", days, w.age_decay_penalty));
        } else {
            let bonus = (days as i32).clamp(0, w.age_bonus_max);
            if bonus > 0 {
                score += bonus;
                breakdown.push(format!("age_{days}d:+{bonus}"));
            }
        }
    }

    // ─── Momentum (recent activity vs dormant) ─────────────────────
    if let Ok(updated) = pr.updated_at.parse::<chrono::DateTime<chrono::Utc>>() {
        let days = chrono::Utc::now().signed_duration_since(updated).num_days();
        if days <= w.recent_days {
            score += w.momentum_recent;
            breakdown.push(format!("active_{days}d:{:+}", w.momentum_recent));
        } else if days >= w.stale_days {
            score += w.momentum_stale;
            breakdown.push(format!("dormant_{days}d:{:+}", w.momentum_stale));
        }
    }

    (score, breakdown)
}

/// Backward-compatible wrapper using the in-code default issue weights.
pub fn score_issue(issue: &Issue, triage: Option<&TriageResultRow>) -> (i32, Vec<String>) {
    score_issue_with(issue, triage, &IssueScoringConfig::default())
}

/// Rank an open issue for triage / planning.
///
/// Combines the AI classification (priority + confidence) with the
/// human signals already on the issue (reactions, labels, recency).
/// Designed to surface what's worth fixing *now*, not just what's loud.
///
/// Signals (defaults; override per-repo via `[scoring.issue]`):
/// - AI priority: critical +50, high +25, medium +10
/// - AI confidence: halve the AI-priority boost if confidence < 0.5
/// - Reactions: +1 per `+1`, capped at 20
/// - Labels: security +30, regression +15, bug +8, good-first-issue +3,
///   blocked/needs-info/wontfix -10
/// - Recency: +5 if updated within 7d, -10 if dormant >90d
pub fn score_issue_with(
    issue: &Issue,
    triage: Option<&TriageResultRow>,
    w: &IssueScoringConfig,
) -> (i32, Vec<String>) {
    let mut score = 0i32;
    let mut breakdown = Vec::new();

    // ─── AI priority (strongest signal when confident) ─────────────
    if let Some(t) = triage {
        let raw = match t.priority.as_deref() {
            Some("critical") => w.ai_priority_critical,
            Some("high") => w.ai_priority_high,
            Some("medium") => w.ai_priority_medium,
            _ => 0,
        };
        if raw != 0 {
            // Damp uncertain classifications so a 30%-confident "critical"
            // doesn't overwhelm a 90%-confident "high" with real reactions.
            let bonus = if t.confidence < w.low_confidence_threshold {
                raw / 2
            } else {
                raw
            };
            score += bonus;
            breakdown.push(format!(
                "ai_{}@{:.0}%:{:+}",
                t.priority.as_deref().unwrap_or("?"),
                t.confidence * 100.0,
                bonus
            ));
        }
    }

    // ─── Community signal: reactions ───────────────────────────────
    let reactions =
        ((issue.reactions_plus1 as i32) * w.reaction_per_plus1).clamp(0, w.reaction_max_bonus);
    if reactions > 0 {
        score += reactions;
        breakdown.push(format!("reactions:+{reactions}"));
    }

    // ─── Label-driven boosts/penalties ─────────────────────────────
    for label in &issue.labels {
        let l = label.to_ascii_lowercase();
        if l == "security" {
            score += w.label_security;
            breakdown.push(format!("security:{:+}", w.label_security));
        } else if l == "regression" {
            score += w.label_regression;
            breakdown.push(format!("regression:{:+}", w.label_regression));
        } else if l == "bug" {
            score += w.label_bug;
            breakdown.push(format!("bug:{:+}", w.label_bug));
        } else if l == "good first issue" || l == "good-first-issue" {
            score += w.label_first_issue;
            breakdown.push(format!("first_issue:{:+}", w.label_first_issue));
        } else if matches!(l.as_str(), "blocked" | "needs-info" | "wontfix") {
            score += w.label_blocked;
            breakdown.push(format!("{l}:{:+}", w.label_blocked));
        }
    }

    // ─── Recency (issues stale fast in fast-moving repos) ──────────
    if let Ok(updated) = issue.updated_at.parse::<chrono::DateTime<chrono::Utc>>() {
        let days = chrono::Utc::now().signed_duration_since(updated).num_days();
        if days <= w.recent_days {
            score += w.recent_bonus;
            breakdown.push(format!("recent_{days}d:{:+}", w.recent_bonus));
        } else if days >= w.stale_days {
            score += w.stale_penalty;
            breakdown.push(format!("dormant_{days}d:{:+}", w.stale_penalty));
        }
    }

    (score, breakdown)
}

fn determine_duplicate_reason(
    _pulls: &[PullRequest],
    indices: &[usize],
    word_sets: &[std::collections::HashSet<String>],
    linked_issues: &[std::collections::HashSet<u64>],
) -> String {
    // Check for shared linked issues
    let mut all_shared: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for i in 0..indices.len() {
        for j in (i + 1)..indices.len() {
            let shared: std::collections::HashSet<_> = linked_issues[indices[i]]
                .intersection(&linked_issues[indices[j]])
                .cloned()
                .collect();
            all_shared.extend(shared);
        }
    }

    if !all_shared.is_empty() {
        let issue_list: Vec<String> = all_shared.iter().map(|n| format!("#{n}")).collect();
        return format!("same linked issue: {}", issue_list.join(", "));
    }

    // Find common words across all titles
    if let Some(first) = indices.first() {
        let common: Vec<String> = word_sets[*first]
            .iter()
            .filter(|w| indices.iter().all(|&idx| word_sets[idx].contains(*w)))
            .cloned()
            .collect();
        if !common.is_empty() {
            return format!("similar titles ({})", common.join(", "));
        }
    }

    "similar content".to_string()
}

// Union-Find
fn find(parent: &mut Vec<usize>, i: usize) -> usize {
    if parent[i] != i {
        parent[i] = find(parent, parent[i]);
    }
    parent[i]
}

fn union(parent: &mut Vec<usize>, i: usize, j: usize) {
    let ri = find(parent, i);
    let rj = find(parent, j);
    if ri != rj {
        parent[ri] = rj;
    }
}

// ---------------------------------------------------------------------------
// Tests — scoring algorithm
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn iso_days_ago(days: i64) -> String {
        let t = chrono::Utc::now() - chrono::Duration::days(days);
        t.to_rfc3339()
    }

    fn pr_fixture() -> PullRequest {
        PullRequest {
            number: 1,
            title: "test".into(),
            body: None,
            state: "open".into(),
            labels: vec![],
            author: Some("alice".into()),
            head_sha: None,
            base_sha: None,
            head_ref: None,
            base_ref: None,
            mergeable: None,
            ci_status: None,
            created_at: iso_days_ago(5),
            updated_at: iso_days_ago(0),
            review_decision: None,
            review_decision_at: None,
            reactions_plus1: 0,
        }
    }

    fn issue_fixture() -> Issue {
        Issue {
            number: 1,
            title: "test".into(),
            body: None,
            state: "open".into(),
            labels: vec![],
            author: Some("alice".into()),
            created_at: iso_days_ago(5),
            updated_at: iso_days_ago(0),
            reactions_plus1: 0,
            reactions_total: 0,
        }
    }

    // ─── score_pr ──────────────────────────────────────────────────

    #[test]
    fn score_pr_green_ci_and_mergeable_beats_threshold() {
        let mut pr = pr_fixture();
        pr.ci_status = Some("success".into());
        pr.mergeable = Some(true);
        let (score, _) = score_pr(&pr);
        // ci+25 mergeable+2 age+5 momentum+5 = 37 > queue threshold 15
        assert!(score >= 30, "expected merge-ready score, got {score}");
    }

    #[test]
    fn score_pr_failed_ci_is_penalised() {
        let mut pr = pr_fixture();
        pr.ci_status = Some("failure".into());
        pr.mergeable = Some(true);
        let (score, breakdown) = score_pr(&pr);
        assert!(score < 0, "failed CI should sink the PR, got {score}");
        assert!(breakdown.iter().any(|b| b.contains("ci_failed")));
    }

    #[test]
    fn score_pr_conflict_dominates_green_ci() {
        let mut pr = pr_fixture();
        pr.ci_status = Some("success".into());
        pr.mergeable = Some(false);
        let (score, _) = score_pr(&pr);
        // ci+25 conflict-25 age+5 momentum+5 = 10 (well below merge threshold 15)
        assert!(
            score < 15,
            "conflict must keep PR below merge threshold, got {score}"
        );
    }

    #[test]
    fn score_pr_blocked_label_overrides_everything() {
        let mut pr = pr_fixture();
        pr.ci_status = Some("success".into());
        pr.mergeable = Some(true);
        pr.labels = vec!["blocked".into()];
        let (score, breakdown) = score_pr(&pr);
        assert!(score < 0, "blocked label must sink PR, got {score}");
        assert!(breakdown.iter().any(|b| b.contains("blocked")));
    }

    #[test]
    fn score_pr_priority_critical_label_boosts() {
        let mut pr_no = pr_fixture();
        pr_no.ci_status = Some("success".into());
        let mut pr_crit = pr_no.clone();
        pr_crit.labels = vec!["priority:critical".into()];
        assert!(score_pr(&pr_crit).0 > score_pr(&pr_no).0 + 25);
    }

    #[test]
    fn score_pr_rotting_pr_is_penalised_not_rewarded() {
        let mut pr = pr_fixture();
        pr.ci_status = Some("success".into());
        pr.mergeable = Some(true);
        pr.created_at = iso_days_ago(120);
        let (score, breakdown) = score_pr(&pr);
        assert!(breakdown.iter().any(|b| b.contains("rotting")));
        // Used to keep climbing forever via uncapped age bonus; now bounded.
        assert!(score < 30);
    }

    #[test]
    fn score_pr_dormant_loses_momentum() {
        let mut pr = pr_fixture();
        pr.ci_status = Some("success".into());
        pr.updated_at = iso_days_ago(45);
        let (_, breakdown) = score_pr(&pr);
        assert!(breakdown.iter().any(|b| b.contains("dormant")));
    }

    #[test]
    fn score_pr_linked_issue_case_insensitive() {
        let mut pr = pr_fixture();
        pr.body = Some("Closes #42".into());
        let (_, breakdown) = score_pr(&pr);
        assert!(breakdown.iter().any(|b| b.contains("linked")));
    }

    // ─── score_issue ───────────────────────────────────────────────

    fn triage_fixture(priority: &str, confidence: f64) -> TriageResultRow {
        TriageResultRow {
            issue_number: 1,
            category: "bug".into(),
            confidence,
            priority: Some(priority.into()),
            summary: None,
            is_simple_fix: false,
            acted_at: chrono::Utc::now().to_rfc3339(),
            content_hash: None,
        }
    }

    #[test]
    fn score_issue_critical_ai_dominates() {
        let i = issue_fixture();
        let t = triage_fixture("critical", 0.95);
        let (score, breakdown) = score_issue(&i, Some(&t));
        assert!(
            score >= 50,
            "critical+confident should rank top, got {score}"
        );
        assert!(breakdown.iter().any(|b| b.contains("ai_critical")));
    }

    #[test]
    fn score_issue_low_confidence_halves_ai_boost() {
        let i = issue_fixture();
        let confident = triage_fixture("high", 0.9);
        let unsure = triage_fixture("high", 0.3);
        assert!(score_issue(&i, Some(&confident)).0 > score_issue(&i, Some(&unsure)).0);
    }

    #[test]
    fn score_issue_security_label_outweighs_low_priority_ai() {
        let mut i = issue_fixture();
        i.labels = vec!["security".into()];
        let t = triage_fixture("low", 0.9);
        // security:+30 alone > AI low (which contributes 0 by default)
        assert!(score_issue(&i, Some(&t)).0 >= 30);
    }

    #[test]
    fn score_issue_reactions_capped() {
        let mut i = issue_fixture();
        i.reactions_plus1 = 9999; // pathologically popular
        let (score, _) = score_issue(&i, None);
        // Without other signals, should be exactly the cap (+ recency bonus
        // because updated_at is "today" in the fixture). Cap is 20.
        assert!(
            score <= 20 + 5,
            "reaction bonus must be capped, got {score}"
        );
    }

    #[test]
    fn score_issue_dormant_penalty_kicks_in() {
        let mut i = issue_fixture();
        i.updated_at = iso_days_ago(180);
        let (_, breakdown) = score_issue(&i, None);
        assert!(breakdown.iter().any(|b| b.contains("dormant")));
    }
}
