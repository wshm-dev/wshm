use anyhow::Result;
use serde::Serialize;

use crate::config::Config;
use crate::db::Database;

#[derive(Serialize)]
struct StatusOutput {
    open_issues: usize,
    untriaged: usize,
    open_prs: usize,
    unanalyzed: usize,
    conflicts: usize,
    last_sync: Option<String>,
}

/// Summary data for the `wshm summary` command.
#[derive(Debug, Serialize)]
pub struct Summary {
    pub repo: String,
    pub timestamp: String,
    pub open_issues: usize,
    pub untriaged_issues: usize,
    pub high_priority_issues: Vec<IssueBrief>,
    pub top_issues: Vec<IssueBrief>,
    pub open_prs: usize,
    pub unanalyzed_prs: usize,
    pub high_risk_prs: Vec<PrBrief>,
    pub top_prs: Vec<PrBrief>,
    pub conflicts: usize,
}

#[derive(Debug, Serialize)]
pub struct IssueBrief {
    pub number: u64,
    pub title: String,
    pub priority: Option<String>,
    pub category: Option<String>,
    pub labels: Vec<String>,
    pub age_days: i64,
}

#[derive(Debug, Serialize)]
pub struct PrBrief {
    pub number: u64,
    pub title: String,
    pub risk_level: Option<String>,
    pub ci_status: Option<String>,
    pub has_conflicts: bool,
    pub age_days: i64,
}

pub fn show(db: &Database, json: bool) -> Result<()> {
    let open_issues = db.get_open_issues()?;
    let untriaged = db.get_untriaged_issues()?;
    let open_pulls = db.get_open_pulls()?;
    let unanalyzed = db.get_unanalyzed_pulls()?;

    let conflicts: usize = open_pulls
        .iter()
        .filter(|p| p.mergeable == Some(false))
        .count();

    let last_sync = db
        .get_sync_entry("issues")
        .ok()
        .flatten()
        .map(|e| e.last_synced_at);

    if json {
        let output = StatusOutput {
            open_issues: open_issues.len(),
            untriaged: untriaged.len(),
            open_prs: open_pulls.len(),
            unanalyzed: unanalyzed.len(),
            conflicts,
            last_sync,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("wshm — status");
    println!("─────────────────────────");
    println!(
        "Issues:  {} open ({} untriaged)",
        open_issues.len(),
        untriaged.len()
    );
    println!(
        "PRs:     {} open ({} unanalyzed)",
        open_pulls.len(),
        unanalyzed.len()
    );

    if conflicts > 0 {
        println!("Conflicts: {conflicts}");
    }

    if let Some(ref sync_time) = last_sync {
        println!("Last sync: {sync_time}");
    } else {
        println!("Last sync: never (run `wshm sync`)");
    }

    Ok(())
}

/// Build a summary from the local database cache.
pub fn build_summary(config: &Config, db: &Database) -> Result<Summary> {
    let open_issues = db.get_open_issues()?;
    let untriaged = db.get_untriaged_issues()?;
    let open_pulls = db.get_open_pulls()?;
    let unanalyzed = db.get_unanalyzed_pulls()?;

    let conflicts = open_pulls
        .iter()
        .filter(|p| p.mergeable == Some(false))
        .count();

    // Collect high/critical priority issues only — sorted oldest first (most urgent)
    let now = chrono::Utc::now();
    let mut high_priority_issues = Vec::new();
    for issue in &open_issues {
        if let Ok(Some(triage)) = db.get_triage_result(issue.number) {
            let is_high = matches!(triage.priority.as_deref(), Some("high") | Some("critical"));
            if !is_high {
                continue;
            }
            let age_days = chrono::DateTime::parse_from_rfc3339(&issue.created_at)
                .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_days())
                .unwrap_or(0);
            high_priority_issues.push(IssueBrief {
                number: issue.number,
                title: crate::pipelines::truncate(&issue.title, 80),
                priority: triage.priority.clone(),
                category: Some(triage.category.clone()),
                labels: issue.labels.clone(),
                age_days,
            });
        }
    }
    // Oldest first = most urgent at the top
    high_priority_issues.sort_by_key(|b| std::cmp::Reverse(b.age_days));

    // Top 10 issues to do — sorted by priority (critical > high > medium > low) then oldest first
    let priority_rank = |p: Option<&str>| match p {
        Some("critical") => 0,
        Some("high") => 1,
        Some("medium") => 2,
        Some("low") => 3,
        _ => 4,
    };
    let mut top_issues = Vec::new();
    for issue in &open_issues {
        let triage = db.get_triage_result(issue.number).ok().flatten();
        let age_days = chrono::DateTime::parse_from_rfc3339(&issue.created_at)
            .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_days())
            .unwrap_or(0);
        top_issues.push(IssueBrief {
            number: issue.number,
            title: crate::pipelines::truncate(&issue.title, 80),
            priority: triage.as_ref().and_then(|t| t.priority.clone()),
            category: triage.as_ref().map(|t| t.category.clone()),
            labels: issue.labels.clone(),
            age_days,
        });
    }
    top_issues.sort_by(|a, b| {
        priority_rank(a.priority.as_deref())
            .cmp(&priority_rank(b.priority.as_deref()))
            .then(b.age_days.cmp(&a.age_days))
    });
    top_issues.truncate(10);

    // Collect high-risk PRs or PRs with conflicts
    let mut high_risk_prs = Vec::new();
    let mut top_prs = Vec::new();
    for pr in &open_pulls {
        let analysis = db.get_pr_analysis(pr.number).ok().flatten();
        let has_conflicts = pr.mergeable == Some(false);
        let age_days = chrono::DateTime::parse_from_rfc3339(&pr.created_at)
            .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_days())
            .unwrap_or(0);

        let brief = PrBrief {
            number: pr.number,
            title: crate::pipelines::truncate(&pr.title, 80),
            risk_level: analysis.as_ref().map(|a| a.risk_level.clone()),
            ci_status: pr.ci_status.clone(),
            has_conflicts,
            age_days,
        };

        let is_high_risk = analysis
            .as_ref()
            .map(|a| a.risk_level == "high")
            .unwrap_or(false);
        if is_high_risk || has_conflicts {
            high_risk_prs.push(PrBrief {
                number: pr.number,
                title: crate::pipelines::truncate(&pr.title, 80),
                risk_level: analysis.map(|a| a.risk_level),
                ci_status: pr.ci_status.clone(),
                has_conflicts,
                age_days,
            });
        }

        top_prs.push(brief);
    }
    // Sort PRs: conflicts first, then oldest first
    top_prs.sort_by(|a, b| {
        b.has_conflicts
            .cmp(&a.has_conflicts)
            .then(b.age_days.cmp(&a.age_days))
    });
    top_prs.truncate(10);

    Ok(Summary {
        repo: config.repo_slug(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        open_issues: open_issues.len(),
        untriaged_issues: untriaged.len(),
        high_priority_issues,
        top_issues,
        open_prs: open_pulls.len(),
        unanalyzed_prs: unanalyzed.len(),
        high_risk_prs,
        top_prs,
        conflicts,
    })
}

/// Format summary output for terminal display.
fn format_terminal(summary: &Summary) -> String {
    let mut output = String::new();

    // Header
    output.push_str(&format!("wshm — {}\n\n", summary.repo));

    // Stats
    output.push_str(&format!(
        "Issues: {} open ({} untriaged)\n",
        summary.open_issues, summary.untriaged_issues
    ));
    output.push_str(&format!(
        "Pull Requests: {} open ({} unanalyzed)\n",
        summary.open_prs, summary.unanalyzed_prs
    ));
    if summary.conflicts > 0 {
        output.push_str(&format!("Conflicts: {}\n", summary.conflicts));
    }

    // Action Required
    if !summary.high_priority_issues.is_empty() {
        output.push_str("\nAction Required:\n");
        for i in summary.high_priority_issues.iter().take(10) {
            let prio = i.priority.as_deref().unwrap_or("?");
            let age = if i.age_days > 0 {
                format!(" ({}d)", i.age_days)
            } else {
                String::new()
            };
            output.push_str(&format!("  #{} {}{} — {}\n", i.number, prio, age, i.title));
        }
    }

    // Attention PRs
    if !summary.high_risk_prs.is_empty() {
        output.push_str("\nAttention PRs:\n");
        for p in summary.high_risk_prs.iter().take(10) {
            let mut tags = Vec::new();
            if let Some(ref risk) = p.risk_level {
                tags.push(format!("risk:{risk}"));
            }
            if p.has_conflicts {
                tags.push("CONFLICT".to_string());
            }
            output.push_str(&format!(
                "  #{} [{}] {}\n",
                p.number,
                tags.join(", "),
                p.title
            ));
        }
    }

    // Issues TODO
    if !summary.top_issues.is_empty() {
        output.push_str("\nIssues TODO:\n");
        for i in &summary.top_issues {
            let prio = i.priority.as_deref().unwrap_or("-");
            let cat = i.category.as_deref().unwrap_or("-");
            let age = if i.age_days > 0 {
                format!(" ({}d)", i.age_days)
            } else {
                String::new()
            };
            output.push_str(&format!(
                "  #{} {}/{}{} — {}\n",
                i.number, prio, cat, age, i.title
            ));
        }
    }

    // PRs TODO
    if !summary.top_prs.is_empty() {
        output.push_str("\nPRs TODO:\n");
        for p in &summary.top_prs {
            let risk = p.risk_level.as_deref().unwrap_or("-");
            let age = if p.age_days > 0 {
                format!(" ({}d)", p.age_days)
            } else {
                String::new()
            };
            let conflict = if p.has_conflicts { " CONFLICT" } else { "" };
            output.push_str(&format!(
                "  #{} {}{}{} — {}\n",
                p.number, risk, conflict, age, p.title
            ));
        }
    }

    output
}

/// Display a daily digest summary (same data format as notifications would send).
pub fn show_summary(config: &Config, db: &Database, json: bool) -> Result<()> {
    let summary = build_summary(config, db)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        print!("{}", format_terminal(&summary));
    }

    Ok(())
}
