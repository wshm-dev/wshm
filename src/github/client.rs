use anyhow::{Context, Result};
use octocrab::Octocrab;
use tracing::debug;

use crate::config::Config;

pub struct Client {
    pub octocrab: Octocrab,
    pub owner: String,
    pub repo: String,
    /// HTML comment marker for idempotent comment updates (from branding.name).
    pub comment_marker: String,
}

impl Client {
    pub fn new(config: &Config) -> Result<Self> {
        let token = config.github_token()?;
        let octocrab = Octocrab::builder()
            .personal_token(token)
            .build()
            .context("Failed to create GitHub client")?;

        Ok(Self {
            octocrab,
            owner: config.repo_owner.clone(),
            repo: config.repo_name.clone(),
            comment_marker: config.branding.comment_marker(),
        })
    }

    /// Check if a user is a collaborator (write access or above) on the repo.
    pub async fn is_collaborator(&self, username: &str) -> Result<bool> {
        // Validate username to prevent URL path injection
        if !username.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            anyhow::bail!("Invalid GitHub username: {username}");
        }
        let url = format!(
            "https://api.github.com/repos/{}/{}/collaborators/{}/permission",
            self.owner, self.repo, username
        );

        let response = self.octocrab._get(&url).await;

        match response {
            Ok(resp) => {
                let body = self.octocrab.body_to_string(resp).await
                    .unwrap_or_else(|e| { tracing::warn!("Failed to read collaborator response: {e}"); String::new() });
                let json: serde_json::Value = serde_json::from_str(&body)
                    .unwrap_or_else(|e| { tracing::warn!("Failed to parse collaborator JSON: {e}"); serde_json::Value::default() });
                let permission = json["permission"].as_str().unwrap_or("none");
                debug!("User {username} permission: {permission}");
                Ok(matches!(permission, "admin" | "write" | "maintain"))
            }
            Err(e) => {
                let err_str = format!("{e}");
                if err_str.contains("404") {
                    // 404 = not a collaborator
                    Ok(false)
                } else {
                    tracing::warn!("Failed to check collaborator status for {username}: {e}");
                    Err(anyhow::anyhow!("Failed to check collaborator status for {username}: {e}"))
                }
            }
        }
    }

    /// Create a draft pull request, returns the PR number.
    pub async fn create_draft_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<u64> {
        let pr_body = serde_json::json!({
            "title": title,
            "body": body,
            "head": head,
            "base": base,
            "draft": true,
        });

        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls",
            self.owner, self.repo
        );

        let response = self
            .octocrab
            ._post(&url, Some(&pr_body))
            .await
            .context("Failed to create draft pull request")?;

        let response_body = self
            .octocrab
            .body_to_string(response)
            .await
            .context("Failed to read create PR response")?;

        let pr_json: serde_json::Value =
            serde_json::from_str(&response_body).context("Failed to parse create PR response")?;

        let number = pr_json["number"]
            .as_u64()
            .context("Missing PR number in response")?;

        Ok(number)
    }
}
