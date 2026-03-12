use anyhow::Result;
use serde::Serialize;

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
