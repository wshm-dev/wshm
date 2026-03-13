use anyhow::{Context, Result};
use tracing::{debug, info};

use crate::db::issues::Issue;
use crate::github::Client;

/// Hidden HTML marker used to detect wshm comments for idempotent updates.
pub const WSHM_COMMENT_MARKER: &str = "<!-- wshm -->";

impl Client {
    pub async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        self.fetch_issues_with_state("open", since).await
    }

    pub async fn fetch_all_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        self.fetch_issues_with_state("all", since).await
    }

    async fn fetch_issues_with_state(
        &self,
        state: &str,
        since: Option<&str>,
    ) -> Result<Vec<Issue>> {
        let mut all_issues = Vec::new();
        let mut page = 1u32;

        loop {
            let mut url = format!(
                "https://api.github.com/repos/{}/{}/issues?state={state}&per_page=100&page={page}",
                self.owner, self.repo
            );
            if let Some(since) = since {
                url.push_str(&format!("&since={since}"));
            }

            let response = self
                .octocrab
                ._get(&url)
                .await
                .context("Failed to fetch issues")?;

            let body = self
                .octocrab
                .body_to_string(response)
                .await
                .context("Failed to read issues response body")?;

            let items: Vec<serde_json::Value> =
                serde_json::from_str(&body).context("Failed to parse issues JSON")?;

            debug!("Fetched page {page} with {} items", items.len());

            if items.is_empty() {
                break;
            }

            for item in &items {
                // Skip PRs (the issues endpoint includes them)
                if item.get("pull_request").is_some() {
                    continue;
                }

                let reactions = item.get("reactions");
                let reactions_plus1 = reactions
                    .and_then(|r| r.get("+1"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let reactions_total = reactions
                    .and_then(|r| r.get("total_count"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;

                let state = item.get("state").and_then(|v| v.as_str()).unwrap_or("open");

                all_issues.push(Issue {
                    number: item["number"].as_u64().unwrap_or(0),
                    title: item["title"].as_str().unwrap_or("").to_string(),
                    body: item.get("body").and_then(|v| v.as_str()).map(String::from),
                    state: state.to_string(),
                    labels: super::extract_labels(item),
                    author: super::extract_author(item),
                    created_at: item["created_at"].as_str().unwrap_or("").to_string(),
                    updated_at: item["updated_at"].as_str().unwrap_or("").to_string(),
                    reactions_plus1,
                    reactions_total,
                });
            }

            if items.len() < 100 {
                break; // Last page
            }
            page += 1;
        }

        Ok(all_issues)
    }

    pub async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .add_labels(number, labels)
            .await
            .with_context(|| format!("Failed to label issue #{number}"))?;
        Ok(())
    }

    /// Post or update a wshm comment on an issue.
    /// If a wshm comment already exists, it is updated in place (idempotent).
    pub async fn comment_issue(&self, number: u64, body: &str) -> Result<()> {
        let body_with_marker = ensure_wshm_marker(body);

        if let Some(comment_id) = self.find_wshm_comment(number).await? {
            info!("Updating existing wshm comment {comment_id} on issue #{number}");
            self.update_comment(comment_id, &body_with_marker).await?;
        } else {
            self.octocrab
                .issues(&self.owner, &self.repo)
                .create_comment(number, &body_with_marker)
                .await
                .with_context(|| format!("Failed to comment on issue #{number}"))?;
        }
        Ok(())
    }

    /// Find an existing wshm comment on an issue/PR by looking for the hidden marker.
    /// Returns `Some(comment_id)` if found, `None` otherwise.
    pub async fn find_wshm_comment(&self, number: u64) -> Result<Option<u64>> {
        let mut page = 1u32;

        loop {
            let url = format!(
                "https://api.github.com/repos/{}/{}/issues/{number}/comments?per_page=100&page={page}",
                self.owner, self.repo
            );

            let response = self
                .octocrab
                ._get(&url)
                .await
                .with_context(|| format!("Failed to fetch comments for issue #{number}"))?;

            let body = self
                .octocrab
                .body_to_string(response)
                .await
                .with_context(|| format!("Failed to read comments response for issue #{number}"))?;

            let comments: Vec<serde_json::Value> =
                serde_json::from_str(&body).context("Failed to parse comments JSON")?;

            if comments.is_empty() {
                break;
            }

            for comment in &comments {
                let comment_body = comment.get("body").and_then(|v| v.as_str()).unwrap_or("");

                if comment_body.contains(WSHM_COMMENT_MARKER) {
                    if let Some(id) = comment.get("id").and_then(|v| v.as_u64()) {
                        return Ok(Some(id));
                    }
                }
            }

            if comments.len() < 100 {
                break;
            }
            page += 1;
        }

        Ok(None)
    }

    /// Update an existing comment by ID.
    pub async fn update_comment(&self, comment_id: u64, body: &str) -> Result<()> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/comments/{comment_id}",
            self.owner, self.repo
        );

        let patch_body = serde_json::json!({ "body": body });

        let response = self
            .octocrab
            ._patch(&url, Some(&patch_body))
            .await
            .with_context(|| format!("Failed to update comment {comment_id}"))?;

        let status = response.status();
        if !status.is_success() {
            let resp_body = self.octocrab.body_to_string(response).await?;
            anyhow::bail!("Failed to update comment {comment_id}: {status} {resp_body}");
        }

        Ok(())
    }

    pub async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/{number}/labels/{label}",
            self.owner, self.repo
        );

        let response = self
            .octocrab
            ._delete(&url, None::<&()>)
            .await
            .with_context(|| format!("Failed to remove label '{label}' from #{number}"))?;

        let status = response.status();
        if !status.is_success() && status.as_u16() != 404 {
            let resp_body = self.octocrab.body_to_string(response).await?;
            anyhow::bail!("Failed to remove label '{label}' from #{number}: {status} {resp_body}");
        }

        Ok(())
    }

    pub async fn close_issue(&self, number: u64) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .update(number)
            .state(octocrab::models::IssueState::Closed)
            .send()
            .await
            .with_context(|| format!("Failed to close issue #{number}"))?;
        Ok(())
    }
}

/// Ensure the comment body contains the hidden wshm marker.
/// If not already present, appends it at the end.
pub fn ensure_wshm_marker(body: &str) -> String {
    if body.contains(WSHM_COMMENT_MARKER) {
        body.to_string()
    } else {
        format!("{body}\n{WSHM_COMMENT_MARKER}")
    }
}
