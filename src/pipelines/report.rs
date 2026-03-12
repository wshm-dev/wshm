use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use crate::cli::ReportArgs;
use crate::db::Database;
use crate::pipelines::pr_health;

#[derive(Serialize)]
struct SlaMetrics {
    avg_pr_age_days: f64,
    oldest_pr_days: i64,
    oldest_pr_number: u64,
    avg_issue_age_days: f64,
    oldest_issue_days: i64,
    oldest_issue_number: u64,
    prs_over_7d: usize,
    prs_over_30d: usize,
    issues_over_7d: usize,
    issues_over_30d: usize,
}

#[derive(Serialize)]
struct ReportData {
    repo: String,
    generated_at: String,
    open_issues: usize,
    untriaged: usize,
    open_prs: usize,
    unanalyzed: usize,
    conflicts: usize,
    triage_results: Vec<TriageRow>,
    pr_analyses: Vec<PrRow>,
    queue: Vec<QueueRow>,
    pr_issue_links: Vec<PrIssueLink>,
    health: pr_health::HealthReport,
    sla: SlaMetrics,
}

#[derive(Serialize)]
struct TriageRow {
    number: u64,
    title: String,
    category: String,
    confidence: f64,
    priority: String,
    summary: String,
    updated_at: String,
    linked_prs: Vec<u64>,
    reactions_plus1: u32,
    reactions_total: u32,
}

#[derive(Serialize)]
struct PrRow {
    number: u64,
    title: String,
    risk_level: String,
    pr_type: String,
    summary: String,
    updated_at: String,
    linked_issues: Vec<u64>,
}

#[derive(Serialize)]
struct QueueRow {
    number: u64,
    title: String,
    score: i32,
    breakdown: String,
    ci: String,
    mergeable: String,
    updated_at: String,
}

#[derive(Serialize)]
struct PrIssueLink {
    pr_number: u64,
    pr_title: String,
    issue_number: u64,
    issue_title: String,
    link_type: String, // "fixes", "closes", "resolves"
}

pub fn run(db: &Database, args: &ReportArgs, repo_name: &str) -> Result<()> {
    let data = gather_data(db, repo_name)?;

    let format = args.format.to_lowercase();

    // JSON format: print to stdout, no file
    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    let default_name = format!(
        "wshm-report.{}",
        match format.as_str() {
            "html" => "html",
            "pdf" => "html",
            _ => "md",
        }
    );
    let output_path = args.output.clone().unwrap_or(default_name);

    let content = match format.as_str() {
        "html" | "pdf" => render_html(&data),
        _ => render_markdown(&data),
    };

    if format == "pdf" {
        let html_path = output_path.replace(".pdf", ".html");
        std::fs::write(&html_path, &content)
            .with_context(|| format!("Failed to write {html_path}"))?;

        let pdf_path = if output_path.ends_with(".pdf") {
            output_path.clone()
        } else {
            format!("{output_path}.pdf")
        };

        convert_to_pdf(&html_path, &pdf_path)?;
        std::fs::remove_file(&html_path).ok();
        println!("Report written to {pdf_path}");
    } else {
        std::fs::write(&output_path, &content)
            .with_context(|| format!("Failed to write {output_path}"))?;
        println!("Report written to {output_path}");
    }

    Ok(())
}

/// Extract linked issue numbers from PR body using "fixes #N", "closes #N", "resolves #N"
fn extract_issue_links(body: &str) -> Vec<(String, u64)> {
    let re = Regex::new(r"(?i)\b(fix(?:es)?|close[sd]?|resolve[sd]?)\s+#(\d+)").unwrap();
    let mut seen = std::collections::HashSet::new();
    re.captures_iter(body)
        .filter_map(|cap| {
            let link_type = cap[1].to_lowercase();
            let num: u64 = cap[2].parse().ok()?;
            if seen.insert(num) {
                Some((link_type, num))
            } else {
                None
            }
        })
        .collect()
}

/// Format a datetime string as relative "Xd ago" or absolute date
fn format_age(datetime: &str) -> String {
    if let Ok(dt) = datetime.parse::<chrono::DateTime<chrono::Utc>>() {
        let days = chrono::Utc::now().signed_duration_since(dt).num_days();
        if days == 0 {
            "today".to_string()
        } else if days == 1 {
            "1d ago".to_string()
        } else if days < 30 {
            format!("{days}d ago")
        } else if days < 365 {
            format!("{}mo ago", days / 30)
        } else {
            format!("{}y ago", days / 365)
        }
    } else {
        datetime.split('T').next().unwrap_or(datetime).to_string()
    }
}

fn gather_data(db: &Database, repo_name: &str) -> Result<ReportData> {
    let open_issues = db.get_open_issues()?;
    let untriaged = db.get_untriaged_issues()?;
    let open_prs = db.get_open_pulls()?;
    let unanalyzed = db.get_unanalyzed_pulls()?;

    let conflicts = open_prs
        .iter()
        .filter(|p| p.mergeable == Some(false))
        .count();

    // Build PR→Issue links map from PR bodies
    let mut pr_to_issues: HashMap<u64, Vec<(String, u64)>> = HashMap::new();
    let mut issue_to_prs: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut all_links: Vec<PrIssueLink> = Vec::new();

    for pr in &open_prs {
        if let Some(ref body) = pr.body {
            let links = extract_issue_links(body);
            for (link_type, issue_num) in &links {
                pr_to_issues
                    .entry(pr.number)
                    .or_default()
                    .push((link_type.clone(), *issue_num));
                issue_to_prs.entry(*issue_num).or_default().push(pr.number);
            }
            for (link_type, issue_num) in links {
                // Find issue title from open issues or from DB
                let issue_title = open_issues
                    .iter()
                    .find(|i| i.number == issue_num)
                    .map(|i| i.title.clone())
                    .unwrap_or_else(|| format!("Issue #{issue_num}"));
                all_links.push(PrIssueLink {
                    pr_number: pr.number,
                    pr_title: pr.title.clone(),
                    issue_number: issue_num,
                    issue_title,
                    link_type,
                });
            }
        }
    }

    // Triage results with updated_at, reactions, and linked PRs
    let triage_results = db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT t.issue_number, i.title, t.category, t.confidence, t.priority, t.summary, i.updated_at, i.reactions_plus1, i.reactions_total
             FROM triage_results t
             JOIN issues i ON i.number = t.issue_number
             ORDER BY t.confidence DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let number: u64 = row.get(0)?;
            Ok((
                number,
                TriageRow {
                    number,
                    title: row.get(1)?,
                    category: row.get(2)?,
                    confidence: row.get(3)?,
                    priority: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    summary: row.get(5)?,
                    updated_at: row.get(6)?,
                    linked_prs: Vec::new(),
                    reactions_plus1: row.get(7)?,
                    reactions_total: row.get(8)?,
                },
            ))
        })?;
        let mut results: Vec<TriageRow> = Vec::new();
        for row in rows {
            let (number, mut triage_row) = row?;
            if let Some(prs) = issue_to_prs.get(&number) {
                triage_row.linked_prs = prs.clone();
            }
            results.push(triage_row);
        }
        Ok::<_, anyhow::Error>(results)
    })?;

    // PR analyses with updated_at and linked issues
    let pr_analyses = db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT a.pr_number, p.title, a.risk_level, a.pr_type, a.summary, p.updated_at
             FROM pr_analyses a
             JOIN pull_requests p ON p.number = a.pr_number
             ORDER BY a.risk_level DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let number: u64 = row.get(0)?;
            Ok((number, PrRow {
                number,
                title: row.get(1)?,
                risk_level: row.get(2)?,
                pr_type: row.get(3)?,
                summary: row.get(4)?,
                updated_at: row.get(5)?,
                linked_issues: Vec::new(),
            }))
        })?;
        let mut results: Vec<PrRow> = Vec::new();
        for row in rows {
            let (number, mut pr_row) = row?;
            if let Some(links) = pr_to_issues.get(&number) {
                pr_row.linked_issues = links.iter().map(|(_, n)| *n).collect();
            }
            results.push(pr_row);
        }
        Ok::<_, anyhow::Error>(results)
    })?;

    // Build queue with score breakdown
    let queue: Vec<QueueRow> = open_prs
        .iter()
        .map(|pr| {
            let mut score = 0i32;
            let mut breakdown_parts = Vec::new();

            if pr.ci_status.as_deref() == Some("success") {
                score += 10;
                breakdown_parts.push("CI:+10".to_string());
            }
            if pr.mergeable == Some(false) {
                score -= 10;
                breakdown_parts.push("conflict:-10".to_string());
            } else if pr.mergeable == Some(true) {
                score += 2;
                breakdown_parts.push("mergeable:+2".to_string());
            }
            if let Ok(created) = pr.created_at.parse::<chrono::DateTime<chrono::Utc>>() {
                let days = chrono::Utc::now().signed_duration_since(created).num_days();
                let age_bonus = days.min(10) as i32;
                if age_bonus > 0 {
                    score += age_bonus;
                    breakdown_parts.push(format!("age:+{age_bonus}"));
                }
            }
            if let Some(ref body) = pr.body {
                if body.contains("fixes #")
                    || body.contains("closes #")
                    || body.contains("resolves #")
                {
                    score += 3;
                    breakdown_parts.push("linked:+3".to_string());
                }
            }

            QueueRow {
                number: pr.number,
                title: pr.title.clone(),
                score,
                breakdown: breakdown_parts.join(" "),
                ci: pr.ci_status.clone().unwrap_or_else(|| "unknown".into()),
                mergeable: match pr.mergeable {
                    Some(true) => "yes".into(),
                    Some(false) => "conflict".into(),
                    None => "unknown".into(),
                },
                updated_at: pr.updated_at.clone(),
            }
        })
        .collect();

    // PR Health analysis
    let health = pr_health::analyze_health(&open_prs, 14);

    // SLA Metrics
    let now = chrono::Utc::now();
    let sla = {
        let pr_ages: Vec<i64> = open_prs
            .iter()
            .filter_map(|pr| {
                pr.created_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .ok()
                    .map(|dt| now.signed_duration_since(dt).num_days())
            })
            .collect();

        let issue_ages: Vec<i64> = open_issues
            .iter()
            .filter_map(|i| {
                i.created_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .ok()
                    .map(|dt| now.signed_duration_since(dt).num_days())
            })
            .collect();

        let avg_pr = if pr_ages.is_empty() {
            0.0
        } else {
            pr_ages.iter().sum::<i64>() as f64 / pr_ages.len() as f64
        };
        let avg_issue = if issue_ages.is_empty() {
            0.0
        } else {
            issue_ages.iter().sum::<i64>() as f64 / issue_ages.len() as f64
        };

        let oldest_pr = pr_ages.iter().copied().max().unwrap_or(0);
        let oldest_pr_num = open_prs
            .iter()
            .filter_map(|pr| {
                pr.created_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .ok()
                    .map(|dt| (pr.number, now.signed_duration_since(dt).num_days()))
            })
            .max_by_key(|&(_, days)| days)
            .map(|(n, _)| n)
            .unwrap_or(0);

        let oldest_issue = issue_ages.iter().copied().max().unwrap_or(0);
        let oldest_issue_num = open_issues
            .iter()
            .filter_map(|i| {
                i.created_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .ok()
                    .map(|dt| (i.number, now.signed_duration_since(dt).num_days()))
            })
            .max_by_key(|&(_, days)| days)
            .map(|(n, _)| n)
            .unwrap_or(0);

        SlaMetrics {
            avg_pr_age_days: avg_pr,
            oldest_pr_days: oldest_pr,
            oldest_pr_number: oldest_pr_num,
            avg_issue_age_days: avg_issue,
            oldest_issue_days: oldest_issue,
            oldest_issue_number: oldest_issue_num,
            prs_over_7d: pr_ages.iter().filter(|&&d| d > 7).count(),
            prs_over_30d: pr_ages.iter().filter(|&&d| d > 30).count(),
            issues_over_7d: issue_ages.iter().filter(|&&d| d > 7).count(),
            issues_over_30d: issue_ages.iter().filter(|&&d| d > 30).count(),
        }
    };

    Ok(ReportData {
        repo: repo_name.to_string(),
        generated_at: chrono::Utc::now()
            .format("%Y-%m-%d %H:%M UTC")
            .to_string(),
        open_issues: open_issues.len(),
        untriaged: untriaged.len(),
        open_prs: open_prs.len(),
        unanalyzed: unanalyzed.len(),
        conflicts,
        triage_results,
        pr_analyses,
        queue,
        pr_issue_links: all_links,
        health,
        sla,
    })
}

fn render_markdown(d: &ReportData) -> String {
    let mut md = String::new();
    md.push_str(&format!("# wshm Report — {}\n\n", d.repo));
    md.push_str(&format!("Generated: {}\n\n", d.generated_at));

    // Overview
    md.push_str("## Overview\n\n");
    md.push_str("| Metric | Count |\n|--------|-------|\n");
    md.push_str(&format!("| Open issues | {} |\n", d.open_issues));
    md.push_str(&format!("| Untriaged | {} |\n", d.untriaged));
    md.push_str(&format!("| Open PRs | {} |\n", d.open_prs));
    md.push_str(&format!("| Unanalyzed PRs | {} |\n", d.unanalyzed));
    md.push_str(&format!("| Conflicts | {} |\n\n", d.conflicts));

    // SLA Tracking
    md.push_str("## SLA Tracking\n\n");
    md.push_str("| Metric | Value |\n|--------|-------|\n");
    md.push_str(&format!(
        "| Avg PR age | {:.1} days |\n",
        d.sla.avg_pr_age_days
    ));
    md.push_str(&format!(
        "| Oldest PR | #{} ({} days) |\n",
        d.sla.oldest_pr_number, d.sla.oldest_pr_days
    ));
    md.push_str(&format!(
        "| PRs > 7 days | {} |\n",
        d.sla.prs_over_7d
    ));
    md.push_str(&format!(
        "| PRs > 30 days | {} |\n",
        d.sla.prs_over_30d
    ));
    md.push_str(&format!(
        "| Avg issue age | {:.1} days |\n",
        d.sla.avg_issue_age_days
    ));
    md.push_str(&format!(
        "| Oldest issue | #{} ({} days) |\n",
        d.sla.oldest_issue_number, d.sla.oldest_issue_days
    ));
    md.push_str(&format!(
        "| Issues > 7 days | {} |\n",
        d.sla.issues_over_7d
    ));
    md.push_str(&format!(
        "| Issues > 30 days | {} |\n\n",
        d.sla.issues_over_30d
    ));

    // Separate linked vs standalone issues
    let linked_issues: Vec<&TriageRow> = d.triage_results.iter().filter(|t| !t.linked_prs.is_empty()).collect();
    let standalone_issues: Vec<&TriageRow> = d.triage_results.iter().filter(|t| t.linked_prs.is_empty()).collect();

    // Linked Issues & PRs (grouped)
    if !linked_issues.is_empty() {
        md.push_str("## Linked Issues & PRs\n\n");
        for t in &linked_issues {
            let reactions_str = if t.reactions_plus1 > 0 {
                format!(" (+1: {})", t.reactions_plus1)
            } else {
                String::new()
            };
            md.push_str(&format!(
                "### Issue #{} — {}{}\n\n",
                t.number,
                escape_md_table(&t.title),
                reactions_str,
            ));
            md.push_str(&format!(
                "- **Category:** {} | **Priority:** {} | **Confidence:** {:.0}% | **Activity:** {}\n",
                t.category,
                t.priority,
                t.confidence * 100.0,
                format_age(&t.updated_at),
            ));
            md.push_str(&format!("- **Summary:** {}\n", escape_md_table(&t.summary)));

            // Show linked PRs
            for pr_num in &t.linked_prs {
                // Find link info
                if let Some(link) = d.pr_issue_links.iter().find(|l| l.pr_number == *pr_num && l.issue_number == t.number) {
                    // Find queue info for this PR
                    let queue_info = d.queue.iter().find(|q| q.number == *pr_num);
                    let mergeable = queue_info.map(|q| q.mergeable.as_str()).unwrap_or("unknown");
                    let score = queue_info.map(|q| q.score).unwrap_or(0);
                    let breakdown = queue_info.map(|q| q.breakdown.as_str()).unwrap_or("");
                    md.push_str(&format!(
                        "- **PR #{} ({}):** {} | Mergeable: **{}** | Score: {} `{}`\n",
                        pr_num,
                        link.link_type,
                        escape_md_table(&link.pr_title),
                        mergeable,
                        score,
                        breakdown,
                    ));
                }
            }
            md.push('\n');
        }
    }

    // Standalone Issues (no linked PR)
    if !standalone_issues.is_empty() {
        md.push_str("## Issues (no linked PR)\n\n");
        md.push_str(
            "| # | Title | Category | Confidence | Priority | +1 | Activity | Summary |\n",
        );
        md.push_str("|---|-------|----------|-----------|----------|-----|----------|--------|\n");
        for t in &standalone_issues {
            let reactions_str = if t.reactions_plus1 > 0 {
                format!("{}", t.reactions_plus1)
            } else {
                "-".to_string()
            };
            md.push_str(&format!(
                "| #{} | {} | {} | {:.0}% | {} | {} | {} | {} |\n",
                t.number,
                escape_md_table(&t.title),
                t.category,
                t.confidence * 100.0,
                t.priority,
                reactions_str,
                format_age(&t.updated_at),
                escape_md_table(&t.summary),
            ));
        }
        md.push('\n');
    }

    // PR Analysis (standalone PRs not linked to issues)
    let standalone_prs: Vec<&PrRow> = d.pr_analyses.iter().filter(|p| p.linked_issues.is_empty()).collect();
    if !standalone_prs.is_empty() {
        md.push_str("## PRs (no linked issue)\n\n");
        md.push_str(
            "| # | Title | Type | Risk | Activity | Summary |\n",
        );
        md.push_str("|---|-------|------|------|----------|--------|\n");
        for p in &standalone_prs {
            md.push_str(&format!(
                "| #{} | {} | {} | {} | {} | {} |\n",
                p.number,
                escape_md_table(&p.title),
                p.pr_type,
                p.risk_level,
                format_age(&p.updated_at),
                escape_md_table(&p.summary),
            ));
        }
        md.push('\n');
    }

    // Merge Queue
    if !d.queue.is_empty() {
        md.push_str("## Merge Queue\n\n");
        md.push_str(
            "| # | Title | Score | Breakdown | CI | Mergeable | Last Activity |\n",
        );
        md.push_str("|---|-------|-------|-----------|----|-----------|--------------|\n");
        for q in &d.queue {
            md.push_str(&format!(
                "| #{} | {} | {} | `{}` | {} | {} | {} |\n",
                q.number,
                escape_md_table(&q.title),
                q.score,
                q.breakdown,
                q.ci,
                q.mergeable,
                format_age(&q.updated_at),
            ));
        }
        md.push('\n');
    }

    // PR Health: Duplicates
    if !d.health.duplicates.is_empty() {
        md.push_str("## Duplicate PRs\n\n");
        for (i, group) in d.health.duplicates.iter().enumerate() {
            md.push_str(&format!(
                "### Group {} — {} (best: #{})\n\n",
                i + 1,
                group.reason,
                group.best
            ));
            md.push_str("| # | Score | Mergeable | Author | Title |\n");
            md.push_str("|---|-------|-----------|--------|-------|\n");
            for m in &group.members {
                let marker = if m.number == group.best { " ★" } else { "" };
                let mergeable_str = match m.mergeable {
                    Some(true) => "yes",
                    Some(false) => "conflict",
                    None => "unknown",
                };
                md.push_str(&format!(
                    "| #{}{} | {} | {} | {} | {} |\n",
                    m.number,
                    marker,
                    m.score,
                    mergeable_str,
                    m.author,
                    escape_md_table(&m.title),
                ));
            }
            md.push('\n');
        }
    }

    // PR Health: Stale
    if !d.health.stale.is_empty() {
        md.push_str("## Stale/Zombie PRs\n\n");
        md.push_str("| # | Days stale | Conflicts | Author | Title |\n");
        md.push_str("|---|-----------|-----------|--------|-------|\n");
        for s in &d.health.stale {
            md.push_str(&format!(
                "| #{} | {} | {} | {} | {} |\n",
                s.number,
                s.days_stale,
                if s.has_conflicts { "yes" } else { "no" },
                s.author,
                escape_md_table(&s.title),
            ));
        }
        md.push('\n');
    }

    md.push_str("---\n*Generated by [wshm](https://github.com/pszymkowiak/wshm)*\n<!-- wshm -->\n");
    md
}

fn render_html(d: &ReportData) -> String {
    let mut h = String::new();
    h.push_str(&format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>wshm Report — {repo}</title>
<style>
  * {{ margin: 0; padding: 0; box-sizing: border-box; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; max-width: 1200px; margin: 0 auto; padding: 2rem; color: #1a1a2e; background: #fafbfc; }}
  h1 {{ font-size: 1.8rem; margin-bottom: 0.3rem; }}
  h2 {{ font-size: 1.3rem; margin: 2rem 0 1rem; border-bottom: 2px solid #e1e4e8; padding-bottom: 0.4rem; }}
  .meta {{ color: #586069; font-size: 0.9rem; margin-bottom: 2rem; }}
  .overview {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(140px, 1fr)); gap: 1rem; margin-bottom: 2rem; }}
  .card {{ background: white; border: 1px solid #e1e4e8; border-radius: 8px; padding: 1rem; text-align: center; }}
  .card .number {{ font-size: 2rem; font-weight: 700; }}
  .card .label {{ font-size: 0.8rem; color: #586069; text-transform: uppercase; letter-spacing: 0.05em; }}
  .card-conflict .number {{ color: #b31d28; }}
  table {{ width: 100%; border-collapse: collapse; background: white; border: 1px solid #e1e4e8; border-radius: 8px; overflow: hidden; margin-bottom: 1.5rem; font-size: 0.85rem; }}
  th {{ background: #f6f8fa; text-align: left; padding: 0.6rem 0.8rem; font-weight: 600; border-bottom: 2px solid #e1e4e8; white-space: nowrap; }}
  td {{ padding: 0.5rem 0.8rem; border-bottom: 1px solid #eaecef; vertical-align: top; }}
  tr:last-child td {{ border-bottom: none; }}
  .badge {{ display: inline-block; padding: 0.15rem 0.5rem; border-radius: 12px; font-size: 0.75rem; font-weight: 600; white-space: nowrap; }}
  .badge-bug {{ background: #fdd; color: #b31d28; }}
  .badge-feature {{ background: #dcffe4; color: #22863a; }}
  .badge-question {{ background: #e1e4e8; color: #586069; }}
  .badge-docs {{ background: #d1ecf1; color: #0c5460; }}
  .badge-duplicate {{ background: #fff3cd; color: #856404; }}
  .badge-wontfix {{ background: #e1e4e8; color: #586069; }}
  .badge-needs-info {{ background: #fff3cd; color: #856404; }}
  .badge-critical {{ background: #b31d28; color: white; }}
  .badge-high {{ background: #fdd; color: #b31d28; }}
  .badge-medium {{ background: #fff5b1; color: #735c0f; }}
  .badge-low {{ background: #dcffe4; color: #22863a; }}
  .badge-yes {{ background: #dcffe4; color: #22863a; }}
  .badge-conflict {{ background: #fdd; color: #b31d28; }}
  .badge-unknown {{ background: #e1e4e8; color: #586069; }}
  .breakdown {{ font-family: 'SF Mono', Monaco, monospace; font-size: 0.75rem; color: #586069; }}
  .age {{ color: #586069; font-size: 0.8rem; white-space: nowrap; }}
  .link-ref {{ font-size: 0.8rem; color: #0366d6; }}
  .link-section {{ margin-bottom: 1.5rem; }}
  .link-card {{ display: flex; align-items: center; gap: 0.5rem; padding: 0.5rem 0.8rem; background: white; border: 1px solid #e1e4e8; border-radius: 6px; margin-bottom: 0.4rem; font-size: 0.85rem; }}
  .link-card .arrow {{ color: #586069; font-weight: 600; }}
  .score {{ font-weight: 700; font-size: 1.1rem; }}
  .score-high {{ color: #22863a; }}
  .score-mid {{ color: #735c0f; }}
  .score-low {{ color: #b31d28; }}
  .footer {{ margin-top: 3rem; padding-top: 1rem; border-top: 1px solid #e1e4e8; font-size: 0.8rem; color: #586069; }}
  @media print {{ body {{ padding: 1rem; }} .card {{ break-inside: avoid; }} }}
</style>
</head>
<body>
<h1>wshm Report</h1>
<p class="meta">{repo} &mdash; {date}</p>

<div class="overview">
  <div class="card"><div class="number">{issues}</div><div class="label">Open Issues</div></div>
  <div class="card"><div class="number">{untriaged}</div><div class="label">Untriaged</div></div>
  <div class="card"><div class="number">{prs}</div><div class="label">Open PRs</div></div>
  <div class="card"><div class="number">{unanalyzed}</div><div class="label">Unanalyzed</div></div>
  <div class="card{conflict_class}"><div class="number">{conflicts}</div><div class="label">Conflicts</div></div>
</div>
"#,
        repo = escape_html(&d.repo),
        date = d.generated_at,
        issues = d.open_issues,
        untriaged = d.untriaged,
        prs = d.open_prs,
        unanalyzed = d.unanalyzed,
        conflicts = d.conflicts,
        conflict_class = if d.conflicts > 0 {
            " card-conflict"
        } else {
            ""
        },
    ));

    // SLA Tracking
    h.push_str("<h2>SLA Tracking</h2>\n<div class=\"overview\">\n");
    h.push_str(&format!(
        "  <div class=\"card\"><div class=\"number\">{:.0}</div><div class=\"label\">Avg PR Age (days)</div></div>\n",
        d.sla.avg_pr_age_days,
    ));
    h.push_str(&format!(
        "  <div class=\"card{}\"><div class=\"number\">{}</div><div class=\"label\">PRs &gt; 7 days</div></div>\n",
        if d.sla.prs_over_7d > 0 { " card-conflict" } else { "" },
        d.sla.prs_over_7d,
    ));
    h.push_str(&format!(
        "  <div class=\"card{}\"><div class=\"number\">{}</div><div class=\"label\">PRs &gt; 30 days</div></div>\n",
        if d.sla.prs_over_30d > 0 { " card-conflict" } else { "" },
        d.sla.prs_over_30d,
    ));
    h.push_str(&format!(
        "  <div class=\"card\"><div class=\"number\">{:.0}</div><div class=\"label\">Avg Issue Age (days)</div></div>\n",
        d.sla.avg_issue_age_days,
    ));
    h.push_str(&format!(
        "  <div class=\"card{}\"><div class=\"number\">{}</div><div class=\"label\">Issues &gt; 30 days</div></div>\n",
        if d.sla.issues_over_30d > 0 { " card-conflict" } else { "" },
        d.sla.issues_over_30d,
    ));
    h.push_str("</div>\n");
    if d.sla.oldest_pr_days > 0 {
        h.push_str(&format!(
            "<p style=\"font-size:0.85rem;color:#586069\">Oldest PR: #{} ({} days) &middot; Oldest issue: #{} ({} days)</p>\n",
            d.sla.oldest_pr_number, d.sla.oldest_pr_days,
            d.sla.oldest_issue_number, d.sla.oldest_issue_days,
        ));
    }

    // Separate linked vs standalone
    let linked_issues: Vec<&TriageRow> = d.triage_results.iter().filter(|t| !t.linked_prs.is_empty()).collect();
    let standalone_issues: Vec<&TriageRow> = d.triage_results.iter().filter(|t| t.linked_prs.is_empty()).collect();

    // Linked Issues & PRs (grouped)
    if !linked_issues.is_empty() {
        h.push_str("<h2>Linked Issues &amp; PRs</h2>\n");
        for t in &linked_issues {
            let reactions_html = if t.reactions_plus1 > 0 {
                format!(" <span class=\"badge\" style=\"background:#e8f0fe;color:#1967d2\">+1: {}</span>", t.reactions_plus1)
            } else {
                String::new()
            };
            h.push_str(&format!(
                "<div style=\"background:white;border:1px solid #e1e4e8;border-radius:8px;padding:1rem;margin-bottom:1rem\">\n\
                 <div style=\"font-weight:600;margin-bottom:0.5rem\">Issue #{} — {}{}</div>\n\
                 <div style=\"font-size:0.85rem;color:#586069;margin-bottom:0.5rem\">\
                 <span class=\"badge badge-{cat}\">{cat}</span> \
                 <span class=\"badge badge-{pri}\">{pri}</span> \
                 {:.0}% confidence &middot; {}\
                 </div>\n\
                 <div style=\"font-size:0.85rem;margin-bottom:0.5rem\">{}</div>\n",
                t.number,
                escape_html(&t.title),
                reactions_html,
                t.confidence * 100.0,
                format_age(&t.updated_at),
                escape_html(&t.summary),
                cat = t.category,
                pri = t.priority,
            ));

            for pr_num in &t.linked_prs {
                if let Some(link) = d.pr_issue_links.iter().find(|l| l.pr_number == *pr_num && l.issue_number == t.number) {
                    let queue_info = d.queue.iter().find(|q| q.number == *pr_num);
                    let mergeable = queue_info.map(|q| q.mergeable.as_str()).unwrap_or("unknown");
                    let mergeable_class = match mergeable {
                        "yes" => "badge-yes",
                        "conflict" => "badge-conflict",
                        _ => "badge-unknown",
                    };
                    let score = queue_info.map(|q| q.score).unwrap_or(0);
                    let breakdown = queue_info.map(|q| q.breakdown.as_str()).unwrap_or("");
                    h.push_str(&format!(
                        "<div style=\"margin-left:1rem;padding:0.4rem 0.6rem;background:#f6f8fa;border-radius:4px;font-size:0.85rem;margin-bottom:0.3rem\">\
                         &rarr; PR #{} <em>({})</em> {} &middot; \
                         <span class=\"badge {}\">mergeable: {}</span> &middot; \
                         score: <strong>{}</strong> <span class=\"breakdown\">{}</span>\
                         </div>\n",
                        pr_num,
                        link.link_type,
                        escape_html(&link.pr_title),
                        mergeable_class,
                        mergeable,
                        score,
                        escape_html(breakdown),
                    ));
                }
            }
            h.push_str("</div>\n");
        }
    }

    // Standalone Issues
    if !standalone_issues.is_empty() {
        h.push_str("<h2>Issues (no linked PR)</h2>\n<table>\n<tr><th>#</th><th>Title</th><th>Category</th><th>Confidence</th><th>Priority</th><th>+1</th><th>Activity</th><th>Summary</th></tr>\n");
        for t in &standalone_issues {
            let reactions_html = if t.reactions_plus1 > 0 {
                format!("<strong>{}</strong>", t.reactions_plus1)
            } else {
                "-".to_string()
            };
            h.push_str(&format!(
                "<tr><td>#{}</td><td>{}</td><td><span class=\"badge badge-{}\">{}</span></td><td>{:.0}%</td><td><span class=\"badge badge-{}\">{}</span></td><td>{}</td><td class=\"age\">{}</td><td>{}</td></tr>\n",
                t.number,
                escape_html(&t.title),
                t.category,
                t.category,
                t.confidence * 100.0,
                t.priority,
                t.priority,
                reactions_html,
                format_age(&t.updated_at),
                escape_html(&t.summary),
            ));
        }
        h.push_str("</table>\n");
    }

    // Standalone PRs (no linked issue)
    let standalone_prs: Vec<&PrRow> = d.pr_analyses.iter().filter(|p| p.linked_issues.is_empty()).collect();
    if !standalone_prs.is_empty() {
        h.push_str("<h2>PRs (no linked issue)</h2>\n<table>\n<tr><th>#</th><th>Title</th><th>Type</th><th>Risk</th><th>Activity</th><th>Summary</th></tr>\n");
        for p in &standalone_prs {
            h.push_str(&format!(
                "<tr><td>#{}</td><td>{}</td><td>{}</td><td><span class=\"badge badge-{}\">{}</span></td><td class=\"age\">{}</td><td>{}</td></tr>\n",
                p.number,
                escape_html(&p.title),
                p.pr_type,
                p.risk_level,
                p.risk_level,
                format_age(&p.updated_at),
                escape_html(&p.summary),
            ));
        }
        h.push_str("</table>\n");
    }

    // Queue table with score breakdown
    if !d.queue.is_empty() {
        h.push_str("<h2>Merge Queue</h2>\n<table>\n<tr><th>#</th><th>Title</th><th>Score</th><th>Breakdown</th><th>CI</th><th>Mergeable</th><th>Activity</th></tr>\n");
        for q in &d.queue {
            let score_class = if q.score >= 15 {
                "score-high"
            } else if q.score >= 5 {
                "score-mid"
            } else {
                "score-low"
            };
            let mergeable_class = match q.mergeable.as_str() {
                "yes" => "badge-yes",
                "conflict" => "badge-conflict",
                _ => "badge-unknown",
            };
            h.push_str(&format!(
                "<tr><td>#{}</td><td>{}</td><td><span class=\"score {}\">{}</span></td><td class=\"breakdown\">{}</td><td>{}</td><td><span class=\"badge {}\">{}</span></td><td class=\"age\">{}</td></tr>\n",
                q.number,
                escape_html(&q.title),
                score_class,
                q.score,
                escape_html(&q.breakdown),
                q.ci,
                mergeable_class,
                q.mergeable,
                format_age(&q.updated_at),
            ));
        }
        h.push_str("</table>\n");
    }

    // PR Health: Duplicates
    if !d.health.duplicates.is_empty() {
        h.push_str("<h2>Duplicate PRs</h2>\n");
        for (i, group) in d.health.duplicates.iter().enumerate() {
            h.push_str(&format!(
                "<div style=\"background:white;border:1px solid #e1e4e8;border-radius:8px;padding:1rem;margin-bottom:1rem\">\n\
                 <div style=\"font-weight:600;margin-bottom:0.5rem\">Group {} — {} <span class=\"badge badge-yes\">best: #{}</span></div>\n\
                 <table style=\"margin:0\"><tr><th>#</th><th>Score</th><th>Mergeable</th><th>Author</th><th>Title</th></tr>\n",
                i + 1,
                escape_html(&group.reason),
                group.best,
            ));
            for m in &group.members {
                let score_class = if m.number == group.best {
                    "score-high"
                } else {
                    "score-low"
                };
                let mergeable_class = match m.mergeable {
                    Some(true) => "badge-yes",
                    Some(false) => "badge-conflict",
                    None => "badge-unknown",
                };
                let mergeable_str = match m.mergeable {
                    Some(true) => "yes",
                    Some(false) => "conflict",
                    None => "unknown",
                };
                h.push_str(&format!(
                    "<tr><td>#{}</td><td><span class=\"score {}\">{}</span></td><td><span class=\"badge {}\">{}</span></td><td>{}</td><td>{}</td></tr>\n",
                    m.number,
                    score_class,
                    m.score,
                    mergeable_class,
                    mergeable_str,
                    escape_html(&m.author),
                    escape_html(&m.title),
                ));
            }
            h.push_str("</table></div>\n");
        }
    }

    // PR Health: Stale
    if !d.health.stale.is_empty() {
        h.push_str("<h2>Stale/Zombie PRs</h2>\n<table>\n<tr><th>#</th><th>Days stale</th><th>Conflicts</th><th>Author</th><th>Title</th></tr>\n");
        for s in &d.health.stale {
            let conflict_class = if s.has_conflicts {
                "badge-conflict"
            } else {
                "badge-yes"
            };
            h.push_str(&format!(
                "<tr><td>#{}</td><td><strong>{}</strong></td><td><span class=\"badge {}\">{}</span></td><td>{}</td><td>{}</td></tr>\n",
                s.number,
                s.days_stale,
                conflict_class,
                if s.has_conflicts { "yes" } else { "no" },
                escape_html(&s.author),
                escape_html(&s.title),
            ));
        }
        h.push_str("</table>\n");
    }

    h.push_str("<div class=\"footer\">Generated by <a href=\"https://github.com/pszymkowiak/wshm\">wshm</a></div>\n</body>\n</html>\n");
    h
}

fn convert_to_pdf(html_path: &str, pdf_path: &str) -> Result<()> {
    // Try wkhtmltopdf first
    let result = std::process::Command::new("wkhtmltopdf")
        .args([
            "--quiet",
            "--enable-local-file-access",
            html_path,
            pdf_path,
        ])
        .output();

    if let Ok(output) = result {
        if output.status.success() {
            return Ok(());
        }
    }

    // Try weasyprint
    let result = std::process::Command::new("weasyprint")
        .args([html_path, pdf_path])
        .output();

    if let Ok(output) = result {
        if output.status.success() {
            return Ok(());
        }
    }

    // Try Chrome/Chromium headless
    for browser in &[
        "google-chrome",
        "chromium",
        "chromium-browser",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ] {
        let result = std::process::Command::new(browser)
            .args([
                "--headless",
                "--disable-gpu",
                &format!("--print-to-pdf={pdf_path}"),
                &format!(
                    "file://{}",
                    Path::new(html_path).canonicalize()?.display()
                ),
            ])
            .output();

        if let Ok(output) = result {
            if output.status.success() {
                return Ok(());
            }
        }
    }

    anyhow::bail!(
        "PDF conversion failed. Install one of: wkhtmltopdf, weasyprint, or Google Chrome.\n\
         The HTML report was generated — use --format html instead."
    )
}

fn escape_md_table(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
