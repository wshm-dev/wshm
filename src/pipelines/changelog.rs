use std::collections::BTreeMap;

use anyhow::Result;

use crate::cli::ChangelogArgs;
use crate::github::pulls::MergedPullRequest;
use crate::github::Client;

/// Categorize a PR title into a changelog section
fn categorize(title: &str) -> &'static str {
    let lower = title.to_lowercase();
    let prefix = lower.split_once(':').map(|(p, _)| p.trim()).unwrap_or("");

    match prefix {
        "feat" | "feature" => "Features",
        "fix" | "bugfix" => "Bug Fixes",
        "docs" | "doc" => "Documentation",
        "refactor" => "Refactoring",
        "chore" | "ci" | "build" => "Maintenance",
        _ => "Other",
    }
}

/// Strip the conventional-commit prefix from a title for display
fn strip_prefix(title: &str) -> &str {
    match title.split_once(':') {
        Some((_, rest)) => rest.trim(),
        None => title,
    }
}

/// Section ordering for consistent output
fn section_order(name: &str) -> u8 {
    match name {
        "Features" => 0,
        "Bug Fixes" => 1,
        "Refactoring" => 2,
        "Documentation" => 3,
        "Maintenance" => 4,
        _ => 5,
    }
}

pub async fn run(gh: &Client, args: &ChangelogArgs) -> Result<()> {
    let since = chrono::Utc::now() - chrono::Duration::days(args.days as i64);
    let since_str = since.format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let merged = gh.fetch_merged_pulls(Some(&since_str)).await?;

    if merged.is_empty() {
        if args.format == "json" {
            println!("{{\"sections\":[]}}");
        } else {
            println!("# Changelog\n\nNo merged PRs in the last {} days.", args.days);
        }
        return Ok(());
    }

    if args.format == "json" {
        print_json(&merged)?;
    } else {
        print_markdown(&merged, args.days);
    }

    Ok(())
}

fn print_markdown(prs: &[MergedPullRequest], days: u64) {
    // Group by category
    let mut sections: BTreeMap<&'static str, Vec<&MergedPullRequest>> = BTreeMap::new();
    for pr in prs {
        let cat = categorize(&pr.title);
        sections.entry(cat).or_default().push(pr);
    }

    // Sort sections by defined order
    let mut ordered: Vec<_> = sections.into_iter().collect();
    ordered.sort_by_key(|(name, _)| section_order(name));

    println!("# Changelog\n");
    println!(
        "_Merged PRs from the last {} day{}._\n",
        days,
        if days == 1 { "" } else { "s" }
    );

    for (section, mut prs) in ordered {
        // Sort by merged_at descending (newest first)
        prs.sort_by(|a, b| b.merged_at.cmp(&a.merged_at));

        println!("## {section}\n");
        for pr in prs {
            let display_title = strip_prefix(&pr.title);
            let author = pr.author.as_deref().unwrap_or("unknown");
            // Extract just the date part from merged_at
            let date = pr.merged_at.split('T').next().unwrap_or(&pr.merged_at);
            println!("- **PR #{number}** — {display_title} (@{author}) — {date}", number = pr.number);
        }
        println!();
    }
}

fn print_json(prs: &[MergedPullRequest]) -> Result<()> {
    use serde_json::json;

    let mut sections: BTreeMap<&str, Vec<serde_json::Value>> = BTreeMap::new();
    for pr in prs {
        let cat = categorize(&pr.title);
        sections.entry(cat).or_default().push(json!({
            "number": pr.number,
            "title": pr.title,
            "author": pr.author,
            "merged_at": pr.merged_at,
            "labels": pr.labels,
        }));
    }

    let mut ordered: Vec<_> = sections.into_iter().collect();
    ordered.sort_by_key(|(name, _)| section_order(name));

    let output = json!({
        "sections": ordered.iter().map(|(name, prs)| {
            json!({
                "name": name,
                "pull_requests": prs,
            })
        }).collect::<Vec<_>>(),
    });

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
