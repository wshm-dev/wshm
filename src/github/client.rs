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
    /// Shared HTTP client for raw requests (diff fetches, etc.)
    pub http: reqwest::Client,
    /// Whether the client was built with a personal token. When `false`,
    /// requests go to GitHub anonymously — public repo reads still work
    /// but the rate limit drops to 60 req/h and any mutating endpoint
    /// (post comment, post label, create PR) will fail with 403/401.
    /// Pipelines that mutate must check this flag and skip with a warning.
    pub authenticated: bool,
}

impl Client {
    pub fn new(config: &Config) -> Result<Self> {
        let token = config.github_token_optional();
        let authenticated = token.is_some();
        let mut builder = Octocrab::builder();
        if let Some(t) = token {
            builder = builder.personal_token(t);
        } else {
            tracing::warn!(
                target: "wshm_core::github",
                "GitHub client built without a token — anonymous mode (60 req/h, public repos read-only). \
                 Add a token in Settings → Secrets for full functionality."
            );
        }
        let octocrab = builder.build().context("Failed to create GitHub client")?;

        let http = reqwest::Client::builder()
            .user_agent("wshm")
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            octocrab,
            owner: config.repo_owner.clone(),
            repo: config.repo_name.clone(),
            comment_marker: config.branding.comment_marker(),
            http,
            authenticated,
        })
    }

    /// GET one page of a GitHub list endpoint, returning the body and the
    /// `rel="next"` URL from the `Link` response header (None on the last
    /// page). Callers paginate by following that URL — never by building
    /// `page=N` URLs, which GitHub rejects on large datasets in favor of
    /// cursor-based pagination (see [`super::parse_link_next`]).
    pub(crate) async fn get_page(
        &self,
        url: &str,
        label: &str,
    ) -> Result<(String, Option<String>)> {
        crate::retry::with_retry(label, || async {
            let response = self
                .octocrab
                ._get(url)
                .await
                .with_context(|| format!("Failed to fetch {label}"))?;
            let next = response
                .headers()
                .get("link")
                .and_then(|v| v.to_str().ok())
                .and_then(super::parse_link_next);
            let body = self
                .octocrab
                .body_to_string(response)
                .await
                .with_context(|| format!("Failed to read {label} response body"))?;
            Ok((body, next))
        })
        .await
    }

    /// Returns Err with a descriptive message when the client is unauthenticated.
    /// Pipelines that mutate the repo (label, comment, create PR) call this
    /// at the top of their function so the daemon logs why an action was
    /// skipped.
    pub fn require_auth(&self, action: &str) -> Result<()> {
        if !self.authenticated {
            anyhow::bail!(
                "{action}: GitHub auth required. Add a github_token in \
                 Settings → Secrets, or set GITHUB_TOKEN."
            );
        }
        Ok(())
    }

    /// Check if a user is a collaborator (write access or above) on the repo.
    pub async fn is_collaborator(&self, username: &str) -> Result<bool> {
        // Validate username to prevent URL path injection
        if !username
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            anyhow::bail!("Invalid GitHub username: {username}");
        }
        let url = format!(
            "https://api.github.com/repos/{}/{}/collaborators/{}/permission",
            self.owner, self.repo, username
        );

        // Retry transient transport failures; a 404 (not a collaborator)
        // is classified as permanent and falls through to the match below.
        let response = crate::retry::with_retry("github: collaborator check", || async {
            let resp = self.octocrab._get(&url).await?;
            let body = self.octocrab.body_to_string(resp).await?;
            Ok::<_, anyhow::Error>(body)
        })
        .await;

        match response {
            Ok(body) => {
                let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|e| {
                    tracing::warn!("Failed to parse collaborator JSON: {e}");
                    serde_json::Value::default()
                });
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
                    Err(anyhow::anyhow!(
                        "Failed to check collaborator status for {username}: {e}"
                    ))
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

        // Connect-only retry: re-issuing after a post-send EOF would create
        // a duplicate draft PR.
        let response_body =
            crate::retry::with_retry_connect_only("github: create draft PR", || async {
                let response = self
                    .octocrab
                    ._post(&url, Some(&pr_body))
                    .await
                    .context("Failed to create draft pull request")?;
                self.octocrab
                    .body_to_string(response)
                    .await
                    .context("Failed to read create PR response")
            })
            .await?;

        let pr_json: serde_json::Value =
            serde_json::from_str(&response_body).context("Failed to parse create PR response")?;

        let number = pr_json["number"]
            .as_u64()
            .context("Missing PR number in response")?;

        Ok(number)
    }
}
