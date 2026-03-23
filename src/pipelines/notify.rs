use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;

use crate::config::Config;
use crate::db::Database;

/// Summary data sent to notification targets.
#[derive(Debug, Serialize)]
pub struct NotifySummary {
    pub repo: String,
    pub timestamp: String,
    pub open_issues: usize,
    pub untriaged_issues: usize,
    pub high_priority_issues: Vec<IssueBrief>,
    pub open_prs: usize,
    pub unanalyzed_prs: usize,
    pub high_risk_prs: Vec<PrBrief>,
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
}

/// Build a summary from the local SQLite cache.
fn build_summary(config: &Config, db: &Database) -> Result<NotifySummary> {
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
            let is_high = matches!(
                triage.priority.as_deref(),
                Some("high") | Some("critical")
            );
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
    high_priority_issues.sort_by(|a, b| b.age_days.cmp(&a.age_days));

    // Collect high-risk PRs or PRs with conflicts
    let mut high_risk_prs = Vec::new();
    for pr in &open_pulls {
        let analysis = db.get_pr_analysis(pr.number).ok().flatten();
        let is_high_risk = analysis
            .as_ref()
            .map(|a| a.risk_level == "high")
            .unwrap_or(false);
        let has_conflicts = pr.mergeable == Some(false);

        if is_high_risk || has_conflicts {
            high_risk_prs.push(PrBrief {
                number: pr.number,
                title: crate::pipelines::truncate(&pr.title, 80),
                risk_level: analysis.map(|a| a.risk_level),
                ci_status: pr.ci_status.clone(),
                has_conflicts,
            });
        }
    }

    Ok(NotifySummary {
        repo: config.repo_slug(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        open_issues: open_issues.len(),
        untriaged_issues: untriaged.len(),
        high_priority_issues,
        open_prs: open_pulls.len(),
        unanalyzed_prs: unanalyzed.len(),
        high_risk_prs,
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
            "value": lines.join("\n"),
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
            "value": lines.join("\n"),
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
                "text": format!("*🚨 Action Required*\n{}", lines.join("\n")),
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
                "text": format!("*Attention PRs*\n{}", lines.join("\n")),
            }
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
        facts.push(serde_json::json!({ "name": "Conflicts", "value": format!("{}", summary.conflicts) }));
    }

    let mut body = vec![
        serde_json::json!({
            "type": "FactSet",
            "facts": facts,
        }),
    ];

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
            "text": format!("**Action Required**\n\n{}", lines.join("\n\n")),
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
            "text": format!("**Attention PRs**\n\n{}", lines.join("\n\n")),
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

pub async fn run(config: &Config, db: &Database, json: bool) -> Result<()> {
    if !config.notify.has_targets() {
        if json {
            println!(r#"{{"status":"no_targets","message":"No notification targets configured. Add [notify.discord], [notify.slack], [notify.teams], or [[notify.webhooks]] to .wshm/config.toml."}}"#);
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
