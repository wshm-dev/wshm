use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;

use super::truncate;
use crate::cli::HealthArgs;
use crate::db::pulls::PullRequest;
use crate::db::Database;

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

pub fn run(db: &Database, args: &HealthArgs, json: bool) -> Result<()> {
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
        members.sort_by(|a, b| b.score.cmp(&a.score));
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
    result.sort_by(|a, b| b.members.len().cmp(&a.members.len()));
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

    stale.sort_by(|a, b| b.days_stale.cmp(&a.days_stale));
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
pub fn score_pr(pr: &PullRequest) -> (i32, Vec<String>) {
    let mut score = 0i32;
    let mut breakdown = Vec::new();

    if pr.ci_status.as_deref() == Some("success") {
        score += 10;
        breakdown.push("CI:+10".to_string());
    }
    if pr.mergeable == Some(false) {
        score -= 10;
        breakdown.push("conflict:-10".to_string());
    } else if pr.mergeable == Some(true) {
        score += 2;
        breakdown.push("mergeable:+2".to_string());
    }
    if let Ok(created) = pr.created_at.parse::<chrono::DateTime<chrono::Utc>>() {
        let days = chrono::Utc::now().signed_duration_since(created).num_days();
        let age_bonus = days.min(10) as i32;
        if age_bonus > 0 {
            score += age_bonus;
            breakdown.push(format!("age:+{age_bonus}"));
        }
    }
    if let Some(ref body) = pr.body {
        if body.contains("fixes #") || body.contains("closes #") || body.contains("resolves #") {
            score += 3;
            breakdown.push("linked:+3".to_string());
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
