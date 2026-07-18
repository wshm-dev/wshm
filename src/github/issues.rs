use anyhow::{Context, Result};
use tracing::{debug, info};

use crate::db::issues::Issue;
use crate::github::Client;

/// Default hidden HTML marker used to detect wshm comments for idempotent updates.
/// The actual marker is derived from branding.name via `BrandingConfig::comment_marker()`.
pub const WSHM_COMMENT_MARKER: &str = "<!-- wshm -->";

/// Build an [`Issue`] from a GitHub API issue JSON object.
///
/// Returns `None` for pull requests, which the issues endpoint also serves
/// (they carry a `pull_request` key).
fn parse_issue(item: &serde_json::Value) -> Option<Issue> {
    if item.get("pull_request").is_some() {
        return None;
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

    Some(Issue {
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
    })
}

impl Client {
    pub async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        self.fetch_issues_with_state("open", since).await
    }

    pub async fn fetch_all_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        self.fetch_issues_with_state("all", since).await
    }

    /// Fetch a single issue by number from the canonical (strongly consistent)
    /// `/issues/{number}` endpoint.
    ///
    /// The list endpoint is served from a replicated index that lags behind
    /// issue creation by seconds, so a list sync triggered right after an
    /// `issues.opened` webhook can miss the brand-new issue. The single-issue
    /// endpoint always reflects the latest state, so we use it to guarantee a
    /// freshly opened issue is in cache before triage.
    ///
    /// Returns `Ok(None)` when the number is a pull request (the issues
    /// endpoint also serves PRs) or the issue was deleted/transferred (404).
    pub async fn fetch_issue(&self, number: u64) -> Result<Option<Issue>> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/{number}",
            self.owner, self.repo
        );

        let body = crate::retry::with_retry("github: fetch issue", || async {
            let response = self
                .octocrab
                ._get(&url)
                .await
                .with_context(|| format!("Failed to fetch issue #{number}"))?;
            self.octocrab
                .body_to_string(response)
                .await
                .context("Failed to read issue response body")
        })
        .await?;

        let item: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse issue JSON")?;

        // A 404 (deleted/transferred issue) comes back as an object with a
        // `message` field rather than an issue payload.
        if item.get("number").is_none() {
            if let Some(msg) = item.get("message").and_then(|v| v.as_str()) {
                if msg.eq_ignore_ascii_case("Not Found") {
                    return Ok(None);
                }
                anyhow::bail!("GitHub error while fetching issue #{number}: {msg}");
            }
            return Ok(None);
        }

        Ok(parse_issue(&item))
    }

    async fn fetch_issues_with_state(
        &self,
        state: &str,
        since: Option<&str>,
    ) -> Result<Vec<Issue>> {
        let mut all_issues = Vec::with_capacity(128);
        let mut url = format!(
            "https://api.github.com/repos/{}/{}/issues?state={state}&per_page={pp}",
            self.owner,
            self.repo,
            pp = super::GITHUB_PER_PAGE
        );
        if let Some(since) = since {
            url.push_str(&format!("&since={since}"));
        }
        let mut page = 1u32;

        loop {
            let (body, next) = self.get_page(&url, "github: fetch issues").await?;
            let items = super::parse_json_array(&body, "issues")?;

            debug!("Fetched page {page} with {} items", items.len());

            for item in &items {
                // Skip PRs (the issues endpoint includes them)
                if let Some(issue) = parse_issue(item) {
                    all_issues.push(issue);
                }
            }

            match next {
                Some(next) if page < super::GITHUB_MAX_PAGES => url = next,
                _ => break, // Last page or safety cap
            }
            page += 1;
        }

        Ok(all_issues)
    }

    pub async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        crate::retry::with_retry("github: label issue", || async {
            self.octocrab
                .issues(&self.owner, &self.repo)
                .add_labels(number, labels)
                .await
                .with_context(|| format!("Failed to label issue #{number}"))?;
            Ok::<_, anyhow::Error>(())
        })
        .await
    }

    /// Add assignees to an issue or PR (GitHub uses the same endpoint).
    pub async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/{number}/assignees",
            self.owner, self.repo
        );
        let body = serde_json::json!({ "assignees": assignees });

        // POST endpoint — use connect-only retry so a post-send EOF doesn't
        // re-issue the write.
        crate::retry::with_retry_connect_only("github: add assignees", || async {
            let response = self
                .octocrab
                ._post(&url, Some(&body))
                .await
                .with_context(|| format!("Failed to assign {assignees:?} to #{number}"))?;

            let status = response.status();
            if !status.is_success() {
                let resp_body = self.octocrab.body_to_string(response).await?;
                anyhow::bail!("Failed to assign #{number}: {status} {resp_body}");
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
    }

    /// Post or update a wshm comment on an issue.
    /// If a wshm comment already exists, it is updated in place (idempotent).
    pub async fn comment_issue(&self, number: u64, body: &str) -> Result<()> {
        let body_with_marker = ensure_comment_marker(body, &self.comment_marker);

        if let Some(comment_id) = self.find_wshm_comment(number, &self.comment_marker).await? {
            info!("Updating existing wshm comment {comment_id} on issue #{number}");
            self.update_comment(comment_id, &body_with_marker).await?;
        } else {
            debug!("Creating new wshm comment on issue #{number}");
            // Connect-only retry: re-issuing a create after a post-send body
            // EOF would duplicate the comment.
            crate::retry::with_retry_connect_only("github: create comment", || async {
                self.octocrab
                    .issues(&self.owner, &self.repo)
                    .create_comment(number, &body_with_marker)
                    .await
                    .with_context(|| format!("Failed to comment on issue #{number}"))?;
                Ok::<_, anyhow::Error>(())
            })
            .await?;
        }
        Ok(())
    }

    /// Find an existing wshm comment on an issue/PR by looking for the hidden marker.
    /// Returns `Some(comment_id)` if found, `None` otherwise.
    /// Searches for both the custom marker and the legacy `<!-- wshm -->` marker.
    pub async fn find_wshm_comment(&self, number: u64, marker: &str) -> Result<Option<u64>> {
        let mut url = format!(
            "https://api.github.com/repos/{}/{}/issues/{number}/comments?per_page={pp}",
            self.owner,
            self.repo,
            pp = super::GITHUB_PER_PAGE
        );
        let mut page = 1u32;

        loop {
            let label = format!("github: fetch comments for issue #{number}");
            let (body, next) = self.get_page(&url, &label).await?;
            let comments = super::parse_json_array(&body, "comments")?;

            for comment in &comments {
                let comment_body = comment.get("body").and_then(|v| v.as_str()).unwrap_or("");

                if comment_body.contains(marker) || comment_body.contains(WSHM_COMMENT_MARKER) {
                    if let Some(id) = comment.get("id").and_then(|v| v.as_u64()) {
                        return Ok(Some(id));
                    }
                }
            }

            match next {
                Some(next) if page < super::GITHUB_MAX_PAGES => url = next,
                _ => break,
            }
            page += 1;
        }

        Ok(None)
    }

    /// Delete a comment by ID.
    pub async fn delete_comment(&self, comment_id: u64) -> Result<()> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/comments/{comment_id}",
            self.owner, self.repo
        );

        crate::retry::with_retry("github: delete comment", || async {
            let response = self
                .octocrab
                ._delete(&url, None::<&()>)
                .await
                .with_context(|| format!("Failed to delete comment {comment_id}"))?;

            let status = response.status();
            if !status.is_success() && status.as_u16() != 404 {
                let resp_body = self.octocrab.body_to_string(response).await?;
                anyhow::bail!("Failed to delete comment {comment_id}: {status} {resp_body}");
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
    }

    /// Update an existing comment by ID.
    pub async fn update_comment(&self, comment_id: u64, body: &str) -> Result<()> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/comments/{comment_id}",
            self.owner, self.repo
        );

        let patch_body = serde_json::json!({ "body": body });

        crate::retry::with_retry("github: update comment", || async {
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
            Ok::<_, anyhow::Error>(())
        })
        .await
    }

    pub async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let encoded_label = urlencoding::encode(label);
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/{number}/labels/{encoded_label}",
            self.owner, self.repo
        );

        crate::retry::with_retry("github: remove label", || async {
            let response = self
                .octocrab
                ._delete(&url, None::<&()>)
                .await
                .with_context(|| format!("Failed to remove label '{label}' from #{number}"))?;

            let status = response.status();
            if !status.is_success() && status.as_u16() != 404 {
                let resp_body = self.octocrab.body_to_string(response).await?;
                anyhow::bail!(
                    "Failed to remove label '{label}' from #{number}: {status} {resp_body}"
                );
            }
            Ok::<_, anyhow::Error>(())
        })
        .await
    }

    pub async fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        let body = ensure_comment_marker(body, &self.comment_marker);
        // Connect-only retry: re-issuing after a post-send EOF would create
        // a duplicate issue.
        let issue = crate::retry::with_retry_connect_only("github: create issue", || async {
            self.octocrab
                .issues(&self.owner, &self.repo)
                .create(title)
                .body(&body)
                .labels(labels.to_vec())
                .send()
                .await
                .with_context(|| format!("Failed to create issue: {title}"))
        })
        .await?;
        Ok(issue.number)
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

/// Ensure the comment body contains a hidden marker for idempotent updates.
/// Checks for both the custom marker and the legacy default marker.
/// If neither is present, appends the custom marker at the end.
pub fn ensure_comment_marker(body: &str, custom_marker: &str) -> String {
    let body = body.trim_end();

    // If body already has a marker (custom or legacy), use as-is
    if body.contains(custom_marker) || body.contains(WSHM_COMMENT_MARKER) {
        body.to_string()
    } else {
        // Append custom marker at the end
        format!("{body}\n\n{custom_marker}")
    }
}

/// Legacy function for backward compatibility - uses default marker.
/// New code should use ensure_comment_marker with a custom marker.
pub fn ensure_wshm_marker(body: &str) -> String {
    ensure_comment_marker(body, WSHM_COMMENT_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_marker_appends_when_missing() {
        let body = "This is a comment";
        let custom_marker = "<!-- MyBot -->";
        let result = ensure_comment_marker(body, custom_marker);
        assert!(result.contains(custom_marker));
        assert!(result.starts_with("This is a comment"));
    }

    #[test]
    fn test_ensure_marker_preserves_when_present() {
        let body = "This is a comment\n\n<!-- MyBot -->";
        let custom_marker = "<!-- MyBot -->";
        let result = ensure_comment_marker(body, custom_marker);
        // Should not duplicate the marker
        assert_eq!(result.matches(custom_marker).count(), 1);
    }

    #[test]
    fn test_ensure_marker_recognizes_legacy() {
        let body = "This is a comment\n\n<!-- wshm -->";
        let custom_marker = "<!-- MyBot -->";
        let result = ensure_comment_marker(body, custom_marker);
        // Should recognize legacy marker and not append custom
        assert!(!result.contains(custom_marker));
        assert!(result.contains("<!-- wshm -->"));
    }

    #[test]
    fn test_ensure_marker_trims_whitespace() {
        let body = "This is a comment\n\n\n   ";
        let custom_marker = "<!-- MyBot -->";
        let result = ensure_comment_marker(body, custom_marker);
        // Should trim trailing whitespace before appending
        assert!(result.contains("comment\n\n<!-- MyBot -->"));
    }

    #[test]
    fn test_legacy_ensure_wshm_marker() {
        let body = "This is a comment";
        let result = ensure_wshm_marker(body);
        assert!(result.contains(WSHM_COMMENT_MARKER));
    }

    #[test]
    fn test_parse_issue_builds_issue() {
        let item = serde_json::json!({
            "number": 338,
            "title": "Something broke",
            "body": "details",
            "state": "open",
            "labels": [{ "name": "bug" }],
            "user": { "login": "octocat" },
            "created_at": "2026-05-01T00:00:00Z",
            "updated_at": "2026-05-02T00:00:00Z",
            "reactions": { "+1": 3, "total_count": 5 },
        });
        let issue = parse_issue(&item).expect("issue should parse");
        assert_eq!(issue.number, 338);
        assert_eq!(issue.title, "Something broke");
        assert_eq!(issue.state, "open");
        assert_eq!(issue.labels, vec!["bug".to_string()]);
        assert_eq!(issue.author.as_deref(), Some("octocat"));
        assert_eq!(issue.reactions_plus1, 3);
        assert_eq!(issue.reactions_total, 5);
    }

    #[test]
    fn test_parse_issue_skips_pull_requests() {
        // The issues endpoint also returns PRs; they carry a `pull_request` key.
        let item = serde_json::json!({
            "number": 339,
            "title": "A pull request",
            "state": "open",
            "pull_request": { "url": "https://api.github.com/..." },
        });
        assert!(parse_issue(&item).is_none());
    }
}
