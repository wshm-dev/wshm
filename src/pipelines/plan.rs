use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::config::GlobalConfig;

#[derive(Serialize)]
pub struct ActionItem {
    pub repo: String,
    pub issue_number: u64,
    pub title: String,
    pub category: String,
    pub priority: String,
    pub summary: String,
    pub is_simple_fix: bool,
    pub age_days: i64,
}

pub struct PlanArgs {
    pub json: bool,
    pub csv: bool,
    pub limit: usize,
}

/// Run the action plan for all configured repos (or a single repo via Config).
pub fn run_multi(global: &GlobalConfig, args: &PlanArgs) -> Result<()> {
    let mut items: Vec<ActionItem> = Vec::new();
    let now = Utc::now();

    for repo_cfg in &global.repos {
        if !repo_cfg.enabled {
            continue;
        }
        let db_path = std::path::PathBuf::from(&repo_cfg.path)
            .join(".wshm")
            .join("state.db");
        if !db_path.exists() {
            continue;
        }
        let conn = match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Cannot open DB for {}: {e}", repo_cfg.slug);
                continue;
            }
        };

        let mut stmt = conn.prepare(
            "SELECT i.number, i.title, i.created_at, t.category, t.priority, t.is_simple_fix, t.summary
             FROM issues i
             LEFT JOIN triage_results t ON i.number = t.issue_number
             WHERE i.state = 'open'
             ORDER BY
                CASE t.priority
                    WHEN 'critical' THEN 0
                    WHEN 'high' THEN 1
                    WHEN 'medium' THEN 2
                    ELSE 3
                END,
                i.created_at ASC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map([args.limit as i64], |row| {
            let created_at: String = row.get(2)?;
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                created_at,
                row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                row.get::<_, Option<String>>(4)?
                    .unwrap_or_else(|| "normal".to_string()),
                row.get::<_, Option<bool>>(5)?.unwrap_or(false),
                row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            ))
        })?;

        for row in rows.flatten() {
            let (number, title, created_at, category, priority, is_simple_fix, summary) = row;
            let age_days = DateTime::parse_from_rfc3339(&created_at)
                .map(|dt| (now - dt.with_timezone(&Utc)).num_days())
                .unwrap_or(0);
            items.push(ActionItem {
                repo: repo_cfg.slug.clone(),
                issue_number: number,
                title,
                category,
                priority,
                summary,
                is_simple_fix,
                age_days,
            });
        }
    }

    output(&items, args)
}

/// Run the action plan for a single repo (using per-repo config).
pub fn run_single(config: &crate::Config, args: &PlanArgs) -> Result<()> {
    let db_path = config.wshm_dir.join("state.db");
    let now = Utc::now();

    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;

    let mut stmt = conn.prepare(
        "SELECT i.number, i.title, i.created_at, t.category, t.priority, t.is_simple_fix, t.summary
         FROM issues i
         LEFT JOIN triage_results t ON i.number = t.issue_number
         WHERE i.state = 'open'
         ORDER BY
            CASE t.priority
                WHEN 'critical' THEN 0
                WHEN 'high' THEN 1
                WHEN 'medium' THEN 2
                ELSE 3
            END,
            i.created_at ASC
         LIMIT ?1",
    )?;

    let slug = config.repo_slug();
    let mut items: Vec<ActionItem> = Vec::new();

    let rows = stmt.query_map([args.limit as i64], |row| {
        let created_at: String = row.get(2)?;
        Ok((
            row.get::<_, u64>(0)?,
            row.get::<_, String>(1)?,
            created_at,
            row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            row.get::<_, Option<String>>(4)?
                .unwrap_or_else(|| "normal".to_string()),
            row.get::<_, Option<bool>>(5)?.unwrap_or(false),
            row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        ))
    })?;

    for row in rows.flatten() {
        let (number, title, created_at, category, priority, is_simple_fix, summary) = row;
        let age_days = DateTime::parse_from_rfc3339(&created_at)
            .map(|dt| (now - dt.with_timezone(&Utc)).num_days())
            .unwrap_or(0);
        items.push(ActionItem {
            repo: slug.clone(),
            issue_number: number,
            title,
            category,
            priority,
            summary,
            is_simple_fix,
            age_days,
        });
    }

    output(&items, args)
}

fn output(items: &[ActionItem], args: &PlanArgs) -> Result<()> {
    if items.is_empty() {
        println!("No action items — all clear.");
        return Ok(());
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(items)?);
        return Ok(());
    }

    if args.csv {
        println!("repo,issue,title,category,priority,age_days,is_simple_fix,summary");
        for item in items {
            println!(
                "{},{},\"{}\",{},{},{},{},\"{}\"",
                item.repo,
                item.issue_number,
                item.title.replace('"', ""),
                item.category,
                item.priority,
                item.age_days,
                item.is_simple_fix,
                item.summary.replace('"', "\"\""),
            );
        }
        return Ok(());
    }

    // Text table
    println!(
        "\n{:<8} {:<20} {:<6} {:<12} {:<5} {:<8} TITLE",
        "PRIORITY", "REPO", "#", "CATEGORY", "AGE", "SIMPLE"
    );
    println!("{}", "─".repeat(100));
    for item in items {
        let age = if item.age_days > 30 {
            format!("{}mo", item.age_days / 30)
        } else {
            format!("{}d", item.age_days)
        };
        let simple = if item.is_simple_fix { "⚡" } else { "" };
        let repo_short = item.repo.split('/').next_back().unwrap_or(&item.repo);
        println!(
            "{:<8} {:<20} #{:<5} {:<12} {:<5} {:<8} {}",
            item.priority,
            &repo_short[..repo_short.len().min(19)],
            item.issue_number,
            &item.category[..item.category.len().min(11)],
            age,
            simple,
            &item.title[..item.title.len().min(60)],
        );
        if !item.summary.is_empty() {
            println!("         {}", &item.summary[..item.summary.len().min(80)]);
        }
    }
    println!(
        "
{} action item(s). Run `wshm fix --issue <N>` to auto-fix eligible items.",
        items.len()
    );
    Ok(())
}
