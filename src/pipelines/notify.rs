use crate::db::backend::DatabaseBackend;
use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;

use crate::config::Config;

/// Summary data sent to notification targets.
#[derive(Debug, Serialize)]
pub struct NotifySummary {
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

/// Build a summary from the local SQLite cache.
fn build_summary(config: &Config, db: &dyn DatabaseBackend) -> Result<NotifySummary> {
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

    Ok(NotifySummary {
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

// ── Formatters ────────────────────────────────────────────────

fn format_discord(summary: &NotifySummary) -> serde_json::Value {
    let mut fields = vec![
        serde_json::json!({
            "name": "Issues",
            "value": format!("{} open ({} untriaged)", summary.open_issues, summary.untriaged_issues),
            "inline": true,
        }),
        serde_json::json!({
            "name": "Pull Requests",
            "value": format!("{} open ({} unanalyzed)", summary.open_prs, summary.unanalyzed_prs),
            "inline": true,
        }),
    ];

    if summary.conflicts > 0 {
        fields.push(serde_json::json!({
            "name": "Conflicts",
            "value": format!("{}", summary.conflicts),
            "inline": true,
        }));
    }

    if !summary.high_priority_issues.is_empty() {
        let lines: Vec<String> = summary
            .high_priority_issues
            .iter()
            .take(10)
            .map(|i| {
                let prio = i.priority.as_deref().unwrap_or("?");
                let age = if i.age_days > 0 {
                    format!(" ({}d)", i.age_days)
                } else {
                    String::new()
                };
                format!("`#{}` **{}**{} — {}", i.number, prio, age, i.title)
            })
            .collect();
        fields.push(serde_json::json!({
            "name": "Action Required",
            "value": lines.join("
        "),
        }));
    }

    if !summary.high_risk_prs.is_empty() {
        let lines: Vec<String> = summary
            .high_risk_prs
            .iter()
            .take(10)
            .map(|p| {
                let mut tags = Vec::new();
                if let Some(ref risk) = p.risk_level {
                    tags.push(format!("risk:{risk}"));
                }
                if p.has_conflicts {
                    tags.push("CONFLICT".to_string());
                }
                format!("`#{}` [{}] {}", p.number, tags.join(", "), p.title)
            })
            .collect();
        fields.push(serde_json::json!({
            "name": "Attention PRs",
            "value": lines.join("
        "),
        }));
    }

    if !summary.top_issues.is_empty() {
        let lines: Vec<String> = summary
            .top_issues
            .iter()
            .map(|i| {
                let prio = i.priority.as_deref().unwrap_or("-");
                let cat = i.category.as_deref().unwrap_or("-");
                let age = if i.age_days > 0 {
                    format!(" ({}d)", i.age_days)
                } else {
                    String::new()
                };
                format!("`#{}` {}/{}{} — {}", i.number, prio, cat, age, i.title)
            })
            .collect();
        fields.push(serde_json::json!({
            "name": "Issues TODO",
            "value": lines.join("
        "),
        }));
    }

    if !summary.top_prs.is_empty() {
        let lines: Vec<String> = summary
            .top_prs
            .iter()
            .map(|p| {
                let risk = p.risk_level.as_deref().unwrap_or("-");
                let age = if p.age_days > 0 {
                    format!(" ({}d)", p.age_days)
                } else {
                    String::new()
                };
                let conflict = if p.has_conflicts { " CONFLICT" } else { "" };
                format!("`#{}` {}{}{} — {}", p.number, risk, conflict, age, p.title)
            })
            .collect();
        fields.push(serde_json::json!({
            "name": "PRs TODO",
            "value": lines.join("
        "),
        }));
    }

    let color = if !summary.high_priority_issues.is_empty() || summary.conflicts > 0 {
        0xE74C3C // red
    } else if summary.untriaged_issues > 0 || summary.unanalyzed_prs > 0 {
        0xF39C12 // orange
    } else {
        0x2ECC71 // green
    };

    serde_json::json!({
        "embeds": [{
            "title": format!("wshm — {}", summary.repo),
            "color": color,
            "fields": fields,
            "footer": { "text": format!("wshm daily summary — {}", &summary.timestamp[..10]) },
        }]
    })
}

fn format_slack(summary: &NotifySummary) -> serde_json::Value {
    let mut blocks = vec![serde_json::json!({
        "type": "header",
        "text": {
            "type": "plain_text",
            "text": format!("wshm — {}", summary.repo),
        }
    })];

    // Stats section
    let mut stats_parts = vec![
        format!(
            "*Issues:* {} open ({} untriaged)",
            summary.open_issues, summary.untriaged_issues
        ),
        format!(
            "*PRs:* {} open ({} unanalyzed)",
            summary.open_prs, summary.unanalyzed_prs
        ),
    ];
    if summary.conflicts > 0 {
        stats_parts.push(format!("*Conflicts:* {}", summary.conflicts));
    }

    blocks.push(serde_json::json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": stats_parts.join("  |  "),
        }
    }));

    if !summary.high_priority_issues.is_empty() {
        blocks.push(serde_json::json!({ "type": "divider" }));
        let lines: Vec<String> = summary
            .high_priority_issues
            .iter()
            .take(10)
            .map(|i| {
                let prio = i.priority.as_deref().unwrap_or("?");
                let age = if i.age_days > 0 {
                    format!(" ({}d)", i.age_days)
                } else {
                    String::new()
                };
                format!("`#{}` *{}*{} — {}", i.number, prio, age, i.title)
            })
            .collect();
        blocks.push(serde_json::json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": format!("*Action Required*
{}", lines.join("
")),
            }
        }));
    }

    if !summary.high_risk_prs.is_empty() {
        blocks.push(serde_json::json!({ "type": "divider" }));
        let lines: Vec<String> = summary
            .high_risk_prs
            .iter()
            .take(10)
            .map(|p| {
                let mut tags = Vec::new();
                if let Some(ref risk) = p.risk_level {
                    tags.push(format!("risk:{risk}"));
                }
                if p.has_conflicts {
                    tags.push("CONFLICT".to_string());
                }
                format!("`#{}` [{}] {}", p.number, tags.join(", "), p.title)
            })
            .collect();
        blocks.push(serde_json::json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": format!("*Attention PRs*
{}", lines.join("
")),
            }
        }));
    }

    if !summary.top_issues.is_empty() {
        blocks.push(serde_json::json!({ "type": "divider" }));
        let lines: Vec<String> = summary
            .top_issues
            .iter()
            .map(|i| {
                let prio = i.priority.as_deref().unwrap_or("-");
                let cat = i.category.as_deref().unwrap_or("-");
                let age = if i.age_days > 0 {
                    format!(" ({}d)", i.age_days)
                } else {
                    String::new()
                };
                format!("`#{}` {}/{}{} — {}", i.number, prio, cat, age, i.title)
            })
            .collect();
        blocks.push(serde_json::json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": format!("*Issues TODO*
{}", lines.join("
")) }
        }));
    }

    if !summary.top_prs.is_empty() {
        blocks.push(serde_json::json!({ "type": "divider" }));
        let lines: Vec<String> = summary
            .top_prs
            .iter()
            .map(|p| {
                let risk = p.risk_level.as_deref().unwrap_or("-");
                let age = if p.age_days > 0 {
                    format!(" ({}d)", p.age_days)
                } else {
                    String::new()
                };
                let conflict = if p.has_conflicts { " CONFLICT" } else { "" };
                format!("`#{}` {}{}{} — {}", p.number, risk, conflict, age, p.title)
            })
            .collect();
        blocks.push(serde_json::json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": format!("*PRs TODO*
{}", lines.join("
")) }
        }));
    }

    blocks.push(serde_json::json!({
        "type": "context",
        "elements": [{
            "type": "mrkdwn",
            "text": format!("wshm daily summary — {}", &summary.timestamp[..10]),
        }]
    }));

    serde_json::json!({ "blocks": blocks })
}

fn format_teams(summary: &NotifySummary) -> serde_json::Value {
    let mut facts = vec![
        serde_json::json!({ "name": "Issues", "value": format!("{} open ({} untriaged)", summary.open_issues, summary.untriaged_issues) }),
        serde_json::json!({ "name": "PRs", "value": format!("{} open ({} unanalyzed)", summary.open_prs, summary.unanalyzed_prs) }),
    ];
    if summary.conflicts > 0 {
        facts.push(
            serde_json::json!({ "name": "Conflicts", "value": format!("{}", summary.conflicts) }),
        );
    }

    let mut body = vec![serde_json::json!({
        "type": "FactSet",
        "facts": facts,
    })];

    if !summary.high_priority_issues.is_empty() {
        let lines: Vec<String> = summary
            .high_priority_issues
            .iter()
            .take(10)
            .map(|i| {
                let prio = i.priority.as_deref().unwrap_or("?");
                let age = if i.age_days > 0 {
                    format!(" ({}d)", i.age_days)
                } else {
                    String::new()
                };
                format!("- `#{}` **{}**{} — {}", i.number, prio, age, i.title)
            })
            .collect();
        body.push(serde_json::json!({
            "type": "TextBlock",
            "text": format!("**Action Required**

{}", lines.join("

")),
            "wrap": true,
        }));
    }

    if !summary.high_risk_prs.is_empty() {
        let lines: Vec<String> = summary
            .high_risk_prs
            .iter()
            .take(10)
            .map(|p| {
                let mut tags = Vec::new();
                if let Some(ref risk) = p.risk_level {
                    tags.push(format!("risk:{risk}"));
                }
                if p.has_conflicts {
                    tags.push("CONFLICT".to_string());
                }
                format!("- `#{}` [{}] {}", p.number, tags.join(", "), p.title)
            })
            .collect();
        body.push(serde_json::json!({
            "type": "TextBlock",
            "text": format!("**Attention PRs**

{}", lines.join("

")),
            "wrap": true,
        }));
    }

    if !summary.top_issues.is_empty() {
        let lines: Vec<String> = summary
            .top_issues
            .iter()
            .map(|i| {
                let prio = i.priority.as_deref().unwrap_or("-");
                let cat = i.category.as_deref().unwrap_or("-");
                let age = if i.age_days > 0 {
                    format!(" ({}d)", i.age_days)
                } else {
                    String::new()
                };
                format!("- `#{}` {}/{}{} — {}", i.number, prio, cat, age, i.title)
            })
            .collect();
        body.push(serde_json::json!({
            "type": "TextBlock",
            "text": format!("**Issues TODO**

{}", lines.join("

")),
            "wrap": true,
        }));
    }

    if !summary.top_prs.is_empty() {
        let lines: Vec<String> = summary
            .top_prs
            .iter()
            .map(|p| {
                let risk = p.risk_level.as_deref().unwrap_or("-");
                let age = if p.age_days > 0 {
                    format!(" ({}d)", p.age_days)
                } else {
                    String::new()
                };
                let conflict = if p.has_conflicts { " CONFLICT" } else { "" };
                format!(
                    "- `#{}` {}{}{} — {}",
                    p.number, risk, conflict, age, p.title
                )
            })
            .collect();
        body.push(serde_json::json!({
            "type": "TextBlock",
            "text": format!("**PRs TODO**

{}", lines.join("

")),
            "wrap": true,
        }));
    }

    let mut card_body = vec![serde_json::json!({
        "type": "TextBlock",
        "size": "Large",
        "weight": "Bolder",
        "text": format!("wshm — {}", summary.repo),
    })];
    card_body.extend(body);

    serde_json::json!({
        "type": "message",
        "attachments": [{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "content": {
                "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                "type": "AdaptiveCard",
                "version": "1.4",
                "body": card_body,
            }
        }]
    })
}

#[allow(dead_code)]
fn format_terminal(summary: &NotifySummary) -> String {
    let mut output = String::new();

    // Header
    output.push_str(&format!(
        "wshm — {}

",
        summary.repo
    ));
    // Stats
    output.push_str(&format!(
        "Issues: {} open ({} untriaged)
",
        summary.open_issues, summary.untriaged_issues
    ));
    output.push_str(&format!(
        "Pull Requests: {} open ({} unanalyzed)
",
        summary.open_prs, summary.unanalyzed_prs
    ));
    if summary.conflicts > 0 {
        output.push_str(&format!(
            "Conflicts: {}
",
            summary.conflicts
        ));
    }
    // Action Required
    if !summary.high_priority_issues.is_empty() {
        output.push_str(
            "
Action Required:
",
        );
        for i in summary.high_priority_issues.iter().take(10) {
            let prio = i.priority.as_deref().unwrap_or("?");
            let age = if i.age_days > 0 {
                format!(" ({}d)", i.age_days)
            } else {
                String::new()
            };
            output.push_str(&format!(
                "  #{} {}{} — {}
",
                i.number, prio, age, i.title
            ));
        }
    }

    // Attention PRs
    if !summary.high_risk_prs.is_empty() {
        output.push_str(
            "
Attention PRs:
",
        );
        for p in summary.high_risk_prs.iter().take(10) {
            let mut tags = Vec::new();
            if let Some(ref risk) = p.risk_level {
                tags.push(format!("risk:{risk}"));
            }
            if p.has_conflicts {
                tags.push("CONFLICT".to_string());
            }
            output.push_str(&format!(
                "  #{} [{}] {}
",
                p.number,
                tags.join(", "),
                p.title
            ));
        }
    }

    // Issues TODO
    if !summary.top_issues.is_empty() {
        output.push_str(
            "
Issues TODO:
",
        );
        for i in &summary.top_issues {
            let prio = i.priority.as_deref().unwrap_or("-");
            let cat = i.category.as_deref().unwrap_or("-");
            let age = if i.age_days > 0 {
                format!(" ({}d)", i.age_days)
            } else {
                String::new()
            };
            output.push_str(&format!(
                "  #{} {}/{}{} — {}
",
                i.number, prio, cat, age, i.title
            ));
        }
    }

    // PRs TODO
    if !summary.top_prs.is_empty() {
        output.push_str(
            "
PRs TODO:
",
        );
        for p in &summary.top_prs {
            let risk = p.risk_level.as_deref().unwrap_or("-");
            let age = if p.age_days > 0 {
                format!(" ({}d)", p.age_days)
            } else {
                String::new()
            };
            let conflict = if p.has_conflicts { " CONFLICT" } else { "" };
            output.push_str(&format!(
                "  #{} {}{}{} — {}
",
                p.number, risk, conflict, age, p.title
            ));
        }
    }

    output
}

// ── Sending ───────────────────────────────────────────────────

async fn send_discord(
    client: &reqwest::Client,
    cfg: &crate::config::DiscordNotifyConfig,
    summary: &NotifySummary,
) -> Result<()> {
    let mut payload = format_discord(summary);
    if let Some(ref username) = cfg.username {
        payload["username"] = serde_json::json!(username);
    }
    if let Some(ref avatar) = cfg.avatar_url {
        payload["avatar_url"] = serde_json::json!(avatar);
    }

    let resp = client
        .post(&cfg.url)
        .json(&payload)
        .send()
        .await
        .context("Discord webhook POST failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Discord webhook returned HTTP {status}: {body}");
    }
    Ok(())
}

async fn send_slack(
    client: &reqwest::Client,
    cfg: &crate::config::SlackNotifyConfig,
    summary: &NotifySummary,
) -> Result<()> {
    let mut payload = format_slack(summary);
    if let Some(ref channel) = cfg.channel {
        payload["channel"] = serde_json::json!(channel);
    }
    if let Some(ref username) = cfg.username {
        payload["username"] = serde_json::json!(username);
    }

    let resp = client
        .post(&cfg.url)
        .json(&payload)
        .send()
        .await
        .context("Slack webhook POST failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Slack webhook returned HTTP {status}: {body}");
    }
    Ok(())
}

async fn send_teams(
    client: &reqwest::Client,
    cfg: &crate::config::TeamsNotifyConfig,
    summary: &NotifySummary,
) -> Result<()> {
    let payload = format_teams(summary);

    let resp = client
        .post(&cfg.url)
        .json(&payload)
        .send()
        .await
        .context("Teams webhook POST failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Teams webhook returned HTTP {status}: {body}");
    }
    Ok(())
}

async fn send_generic(
    client: &reqwest::Client,
    cfg: &crate::config::GenericNotifyWebhook,
    summary: &NotifySummary,
) -> Result<()> {
    let payload = serde_json::to_vec(summary)?;

    let mut req = client
        .post(&cfg.url)
        .header("Content-Type", "application/json")
        .header("User-Agent", "wshm-notify/1.0");

    if let Some(ref secret) = cfg.secret {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
        mac.update(&payload);
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        req = req.header("X-Wshm-Signature", sig);
    }

    let resp = req
        .body(payload)
        .send()
        .await
        .with_context(|| format!("Generic webhook POST to {} failed", cfg.url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        anyhow::bail!("Webhook {} returned HTTP {status}", cfg.url);
    }
    Ok(())
}

// ── Public API ────────────────────────────────────────────────

pub async fn run(config: &Config, db: &dyn DatabaseBackend, json: bool) -> Result<()> {
    if !config.notify.has_targets() {
        if json {
            println!(
                r#"{{"status":"no_targets","message":"No notification targets configured. Add [notify.discord], [notify.slack], [notify.teams], or [[notify.webhooks]] to .wshm/config.toml."}}"#
            );
        } else {
            println!("No notification targets configured.");
            println!("Add [notify.discord], [notify.slack], [notify.teams], or [[notify.webhooks]] to .wshm/config.toml.");
        }
        return Ok(());
    }

    let summary = build_summary(config, db)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    }

    let client = reqwest::Client::new();
    let mut sent = 0u32;
    let mut errors = 0u32;

    for cfg in &config.notify.discord {
        match send_discord(&client, cfg, &summary).await {
            Ok(()) => {
                sent += 1;
                tracing::info!("Notification sent to Discord");
            }
            Err(e) => {
                errors += 1;
                tracing::warn!("Discord notification failed: {e:#}");
            }
        }
    }

    for cfg in &config.notify.slack {
        match send_slack(&client, cfg, &summary).await {
            Ok(()) => {
                sent += 1;
                tracing::info!("Notification sent to Slack");
            }
            Err(e) => {
                errors += 1;
                tracing::warn!("Slack notification failed: {e:#}");
            }
        }
    }

    for cfg in &config.notify.teams {
        match send_teams(&client, cfg, &summary).await {
            Ok(()) => {
                sent += 1;
                tracing::info!("Notification sent to Teams");
            }
            Err(e) => {
                errors += 1;
                tracing::warn!("Teams notification failed: {e:#}");
            }
        }
    }

    for cfg in &config.notify.webhooks {
        match send_generic(&client, cfg, &summary).await {
            Ok(()) => {
                sent += 1;
                tracing::info!("Notification sent to webhook {}", cfg.url);
            }
            Err(e) => {
                errors += 1;
                tracing::warn!("Webhook notification failed: {e:#}");
            }
        }
    }

    if !json {
        if errors == 0 {
            println!("Notifications sent: {sent} target(s).");
        } else {
            println!("Notifications: {sent} sent, {errors} failed.");
        }
    }

    Ok(())
}

/// Display the daily summary in the terminal (same data as notification targets).
#[allow(dead_code)]
pub fn show_summary(config: &Config, db: &dyn DatabaseBackend, json: bool) -> Result<()> {
    let summary = build_summary(config, db)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        print!("{}", format_terminal(&summary));
    }

    Ok(())
}

// ── Alert notifications (for daemon failures) ────────────────

/// Send a simple alert message to all configured notification targets.
/// Used by the daemon scheduler when sync or AI calls fail repeatedly.
// TODO: re-wire via crate::pro_hooks once a notify hook is added in OSS;
// for now the code is preserved but unreachable.
#[allow(dead_code)]
pub async fn send_alert(config: &Config, title: &str, message: &str, level: AlertLevel) {
    if !config.notify.has_targets() {
        return;
    }

    let client = reqwest::Client::new();

    for cfg in &config.notify.discord {
        if let Err(e) = send_discord_alert(&client, cfg, title, message, level).await {
            tracing::warn!("Discord alert failed: {e:#}");
        }
    }

    for cfg in &config.notify.slack {
        if let Err(e) = send_slack_alert(&client, cfg, title, message).await {
            tracing::warn!("Slack alert failed: {e:#}");
        }
    }

    for cfg in &config.notify.teams {
        if let Err(e) = send_teams_alert(&client, cfg, title, message).await {
            tracing::warn!("Teams alert failed: {e:#}");
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum AlertLevel {
    Warning,
    Critical,
    Resolved,
}

#[allow(dead_code)]
async fn send_discord_alert(
    client: &reqwest::Client,
    cfg: &crate::config::DiscordNotifyConfig,
    title: &str,
    message: &str,
    level: AlertLevel,
) -> Result<()> {
    let (color, icon) = match level {
        AlertLevel::Warning => (0xFFA500, " {26A0} {FE0F}"), // orange, warning
        AlertLevel::Critical => (0xFF0000, " {1F6A8}"),      // red, siren
        AlertLevel::Resolved => (0x00FF00, " {2705}"),       // green, check
    };
    let payload = serde_json::json!({
        "username": cfg.username.as_deref().unwrap_or("wshm"),
        "avatar_url": cfg.avatar_url,
        "embeds": [{
            "title": format!("{icon} {title}"),
            "description": message,
            "color": color,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "footer": { "text": "wshm daemon" }
        }]
    });

    let resp = client
        .post(&cfg.url)
        .json(&payload)
        .send()
        .await
        .context("Discord alert POST failed")?;

    if !resp.status().is_success() {
        let status = resp.status();
        anyhow::bail!("Discord alert returned HTTP {status}");
    }
    Ok(())
}

#[allow(dead_code)]
async fn send_slack_alert(
    client: &reqwest::Client,
    cfg: &crate::config::SlackNotifyConfig,
    title: &str,
    message: &str,
) -> Result<()> {
    let payload = serde_json::json!({
        "channel": cfg.channel,
        "text": format!("*{title}*
    {message}"),
    });
    client
        .post(&cfg.url)
        .json(&payload)
        .send()
        .await
        .context("Slack alert POST failed")?;
    Ok(())
}

#[allow(dead_code)]
async fn send_teams_alert(
    client: &reqwest::Client,
    cfg: &crate::config::TeamsNotifyConfig,
    title: &str,
    message: &str,
) -> Result<()> {
    let payload = serde_json::json!({
        "title": title,
        "text": message,
    });

    client
        .post(&cfg.url)
        .json(&payload)
        .send()
        .await
        .context("Teams alert POST failed")?;
    Ok(())
}
