use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::db::pulls::PullRequest;
use crate::github::issues::ensure_wshm_marker;
use crate::github::Client;

/// A merged pull request with its merge date
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedPullRequest {
    pub number: u64,
    pub title: String,
    pub body: Option<String>,
    pub author: Option<String>,
    pub labels: Vec<String>,
    pub merged_at: String,
    pub created_at: String,
}

impl Client {
    /// Fetch merged pull requests, optionally filtering to those merged since a given ISO date.
    pub async fn fetch_merged_pulls(&self, since: Option<&str>) -> Result<Vec<MergedPullRequest>> {
        let mut all = Vec::with_capacity(128);
        let mut page = 1u32;

        loop {
            let url = format!(
                "https://api.github.com/repos/{}/{}/pulls?state=closed&sort=updated&direction=desc&per_page={pp}&page={page}",
                self.owner, self.repo, pp = super::GITHUB_PER_PAGE
            );

            let response = self
                .octocrab
                ._get(&url)
                .await
                .context("Failed to fetch closed pull requests")?;

            let body = self
                .octocrab
                .body_to_string(response)
                .await
                .context("Failed to read closed pulls response body")?;

            let items: Vec<serde_json::Value> =
                serde_json::from_str(&body).context("Failed to parse closed pulls JSON")?;

            if items.is_empty() {
                break;
            }

            let mut stop = false;
            for pr in &items {
                // Only include actually merged PRs
                let merged_at = match pr.get("merged_at").and_then(|v| v.as_str()) {
                    Some(date) => date.to_string(),
                    None => continue,
                };

                // If we have a since cutoff, skip PRs merged before it
                if let Some(cutoff) = since {
                    if merged_at.as_str() < cutoff {
                        stop = true;
                        break;
                    }
                }

                all.push(MergedPullRequest {
                    number: pr["number"].as_u64().unwrap_or(0),
                    title: pr["title"].as_str().unwrap_or("").to_string(),
                    body: pr.get("body").and_then(|v| v.as_str()).map(String::from),
                    author: super::extract_author(pr),
                    labels: super::extract_labels(pr),
                    merged_at,
                    created_at: pr["created_at"].as_str().unwrap_or("").to_string(),
                });
            }

            if stop || items.len() < 100 {
                break;
            }
            page += 1;
        }

        Ok(all)
    }

    pub async fn fetch_pulls(&self) -> Result<Vec<PullRequest>> {
        let mut all_pulls = Vec::with_capacity(64);
        let mut page = 1u32;

        loop {
            let url = format!(
                "https://api.github.com/repos/{}/{}/pulls?state=open&per_page={pp}&page={page}",
                self.owner, self.repo, pp = super::GITHUB_PER_PAGE
            );

            let response = self
                .octocrab
                ._get(&url)
                .await
                .context("Failed to fetch pull requests")?;

            let body = self
                .octocrab
                .body_to_string(response)
                .await
                .context("Failed to read pulls response body")?;

            let items: Vec<serde_json::Value> =
                serde_json::from_str(&body).context("Failed to parse pulls JSON")?;

            if items.is_empty() {
                break;
            }

            for pr in &items {
                let state = pr.get("state").and_then(|v| v.as_str()).unwrap_or("open");
                let mergeable = pr.get("mergeable").and_then(|v| v.as_bool());

                all_pulls.push(PullRequest {
                    number: pr["number"].as_u64().unwrap_or(0),
                    title: pr["title"].as_str().unwrap_or("").to_string(),
                    body: pr.get("body").and_then(|v| v.as_str()).map(String::from),
                    state: state.to_string(),
                    labels: super::extract_labels(pr),
                    author: super::extract_author(pr),
                    head_sha: pr
                        .get("head")
                        .and_then(|h| h.get("sha"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    base_sha: pr
                        .get("base")
                        .and_then(|h| h.get("sha"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    head_ref: pr
                        .get("head")
                        .and_then(|h| h.get("ref"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    base_ref: pr
                        .get("base")
                        .and_then(|h| h.get("ref"))
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    mergeable,
                    ci_status: None,
                    created_at: pr["created_at"].as_str().unwrap_or("").to_string(),
                    updated_at: pr["updated_at"].as_str().unwrap_or("").to_string(),
                });
            }

            if items.len() < 100 {
                break; // Last page
            }
            page += 1;
        }

        Ok(all_pulls)
    }

    /// Fetch mergeable status for a single PR (requires individual API call)
    pub async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{number}",
            self.owner, self.repo
        );
        let response = self
            .octocrab
            ._get(&url)
            .await
            .with_context(|| format!("Failed to fetch PR #{number} details"))?;

        let body = self
            .octocrab
            .body_to_string(response)
            .await
            .with_context(|| format!("Failed to read PR #{number} body"))?;

        let pr_json: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse PR JSON")?;

        Ok(pr_json.get("mergeable").and_then(|v| v.as_bool()))
    }

    pub async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{number}",
            self.owner, self.repo
        );

        let response = self
            .octocrab
            ._get(&url)
            .await
            .with_context(|| format!("Failed to fetch diff for PR #{number}"))?;

        let text = self
            .octocrab
            .body_to_string(response)
            .await
            .with_context(|| format!("Failed to read diff body for PR #{number}"))?;

        Ok(text)
    }

    /// Fetch the raw unified diff for a PR
    pub async fn fetch_pr_diff_raw(&self, number: u64) -> Result<String> {
        // Use the .diff URL which returns raw unified diff
        let url = format!(
            "https://github.com/{}/{}/pull/{number}.diff",
            self.owner, self.repo
        );

        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .header("User-Agent", "wshm")
            .send()
            .await
            .with_context(|| format!("Failed to fetch raw diff for PR #{number}"))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .with_context(|| format!("Failed to read raw diff for PR #{number}"))?;

        if !status.is_success() {
            anyhow::bail!("Failed to fetch diff for PR #{number}: HTTP {status}");
        }

        Ok(text)
    }

    /// Submit a review with inline comments on a PR
    pub async fn submit_review(
        &self,
        number: u64,
        body: &str,
        comments: &[(String, u64, String)], // (path, line, body)
    ) -> Result<()> {
        let review_comments: Vec<serde_json::Value> = comments
            .iter()
            .map(|(path, line, comment_body)| {
                serde_json::json!({
                    "path": path,
                    "line": line,
                    "body": comment_body,
                })
            })
            .collect();

        let event = if comments.is_empty() {
            "COMMENT"
        } else {
            "COMMENT"
        };

        let review_body = serde_json::json!({
            "body": body,
            "event": event,
            "comments": review_comments,
        });

        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{number}/reviews",
            self.owner, self.repo
        );

        let response = self
            .octocrab
            ._post(&url, Some(&review_body))
            .await
            .with_context(|| format!("Failed to submit review on PR #{number}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = self.octocrab.body_to_string(response).await?;
            anyhow::bail!("Failed to submit review on PR #{number}: {status} {body}");
        }

        Ok(())
    }

    /// Create a new pull request, returns the PR number
    pub async fn create_pr(&self, title: &str, body: &str, head: &str, base: &str) -> Result<u64> {
        let pr_body = serde_json::json!({
            "title": title,
            "body": body,
            "head": head,
            "base": base,
        });

        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls",
            self.owner, self.repo
        );

        let response = self
            .octocrab
            ._post(&url, Some(&pr_body))
            .await
            .context("Failed to create pull request")?;

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

    pub async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .add_labels(number, labels)
            .await
            .with_context(|| format!("Failed to label PR #{number}"))?;
        Ok(())
    }

    /// Post or update a wshm comment on a PR.
    /// If a wshm comment already exists, it is updated in place (idempotent).
    pub async fn comment_pr(&self, number: u64, body: &str) -> Result<()> {
        let body_with_marker = ensure_wshm_marker(body);

        // PRs use the issues comments API on GitHub
        if let Some(comment_id) = self.find_wshm_comment(number).await? {
            info!("Updating existing wshm comment {comment_id} on PR #{number}");
            self.update_comment(comment_id, &body_with_marker).await?;
        } else {
            self.octocrab
                .issues(&self.owner, &self.repo)
                .create_comment(number, &body_with_marker)
                .await
                .with_context(|| format!("Failed to comment on PR #{number}"))?;
        }
        Ok(())
    }
}
