use anyhow::Result;
use serde::Serialize;
use tracing::info;

use crate::cli::ConflictArgs;
use crate::config::Config;
use crate::db::Database;
use crate::export::ExportManager;
use crate::github::Client as GhClient;

#[derive(Serialize)]
struct ConflictScanOutput {
    total: usize,
    clean: usize,
    conflicts: Vec<ConflictPr>,
    unknown: usize,
}

#[derive(Serialize)]
struct ConflictPr {
    number: u64,
    title: String,
    base_ref: Option<String>,
    head_ref: Option<String>,
}

pub async fn run(
    _config: &Config,
    db: &Database,
    _gh: &GhClient,
    _args: &ConflictArgs,
    json: bool,
    _exporter: Option<&ExportManager>,
) -> Result<()> {
    let pulls = db.get_open_pulls()?;

    if pulls.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&ConflictScanOutput {
                    total: 0,
                    clean: 0,
                    conflicts: Vec::new(),
                    unknown: 0,
                })?
            );
        } else {
            println!("No open PRs to check for conflicts.");
        }
        return Ok(());
    }

    let mut conflict_prs = Vec::new();
    let mut clean = 0;
    let mut unknown = 0;

    for pr in &pulls {
        match pr.mergeable {
            Some(true) => clean += 1,
            Some(false) => {
                conflict_prs.push(pr);
            }
            None => unknown += 1,
        }
    }

    if json {
        let output = ConflictScanOutput {
            total: pulls.len(),
            clean,
            conflicts: conflict_prs
                .iter()
                .map(|pr| ConflictPr {
                    number: pr.number,
                    title: pr.title.clone(),
                    base_ref: pr.base_ref.clone(),
                    head_ref: pr.head_ref.clone(),
                })
                .collect(),
            unknown,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("Conflict Scan ({} PRs):", pulls.len());
    println!("  Clean: {clean}");
    println!("  Conflicts: {}", conflict_prs.len());
    println!("  Unknown: {unknown}");

    if !conflict_prs.is_empty() {
        println!("\nConflicting PRs:");
        for pr in &conflict_prs {
            println!(
                "  #{} {} ({}←{})",
                pr.number,
                pr.title,
                pr.base_ref.as_deref().unwrap_or("?"),
                pr.head_ref.as_deref().unwrap_or("?"),
            );
        }
    }

    // TODO: auto-resolve with AI in M3
    if !conflict_prs.is_empty() {
        info!("Auto-resolve not yet implemented. Coming in M3.");
    }

    Ok(())
}
