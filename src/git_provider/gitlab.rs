use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::info;

use crate::config::Config;
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;

use super::{GitProvider, MergedPr};

/// GitLab provider — uses GitLab REST API v4.
/// Supports GitLab.com and self-hosted instances.
pub struct GitLabProvider {
    http: reqwest::Client,
    base_url: String,
    project_path: String, // URL-encoded "owner/repo"
    token: String,
}

impl GitLabProvider {
    pub fn new(config: &Config, base_url: Option<&str>) -> Result<Self> {
        let token = std::env::var("GITLAB_TOKEN")
            .or_else(|_| std::env::var("WSHM_TOKEN"))
            .context("No GitLab token found. Set GITLAB_TOKEN or WSHM_TOKEN.")?;

        let base = base_url.unwrap_or("https://gitlab.com");
        let project_path = urlencoding::encode(&config.repo_slug()).to_string();

        let http = reqwest::Client::builder()
            .user_agent("wshm")
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        info!("GitLab provider initialized: {base}/api/v4/projects/{project_path}");

        Ok(Self {
            http,
            base_url: base.trim_end_matches('/').to_string(),
            project_path,
            token,
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!(
            "{}/api/v4/projects/{}/{}",
            self.base_url, self.project_path, path
        )
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let url = self.api_url(path);
        let resp = self
            .http
            .get(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .send()
            .await
            .with_context(|| format!("GitLab API GET {path}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!(
                "GitLab API error ({status}): {}",
                &text[..text.len().min(200)]
            );
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn post(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = self.api_url(path);
        let resp = self
            .http
            .post(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("GitLab API POST {path}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!(
                "GitLab API error ({status}): {}",
                &text[..text.len().min(200)]
            );
        }
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }

    async fn put(&self, path: &str, body: &serde_json::Value) -> Result<()> {
        let url = self.api_url(path);
        let resp = self
            .http
            .put(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .json(body)
            .send()
            .await
            .with_context(|| format!("GitLab API PUT {path}"))?;
        if !resp.status().is_success() {
            let text = resp.text().await?;
            anyhow::bail!("GitLab API error: {}", &text[..text.len().min(200)]);
        }
        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let url = self.api_url(path);
        let resp = self
            .http
            .delete(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 404 {
            let text = resp.text().await?;
            anyhow::bail!("GitLab API DELETE error: {}", &text[..text.len().min(200)]);
        }
        Ok(())
    }
}

#[async_trait]
impl GitProvider for GitLabProvider {
    fn provider_name(&self) -> &str {
        "gitlab"
    }

    fn repo_slug(&self) -> String {
        urlencoding::decode(&self.project_path)
            .unwrap_or_default()
            .to_string()
    }

    async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        let mut path = "issues?state=opened&per_page=100".to_string();
        if let Some(since) = since {
            path.push_str(&format!("&updated_after={since}"));
        }
        let items = self.get(&path).await?;
        let arr = items.as_array().context("Expected array")?;
        Ok(arr
            .iter()
            .map(|i| Issue {
                number: i["iid"].as_u64().unwrap_or(0),
                title: i["title"].as_str().unwrap_or("").to_string(),
                body: i["description"].as_str().map(String::from),
                state: "open".to_string(),
                labels: i["labels"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                author: i["author"]["username"].as_str().map(String::from),
                created_at: i["created_at"].as_str().unwrap_or("").to_string(),
                updated_at: i["updated_at"].as_str().unwrap_or("").to_string(),
                reactions_plus1: 0,
                reactions_total: 0,
            })
            .collect())
    }

    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        let current = self.get(&format!("issues/{number}")).await?;
        let mut existing: Vec<String> = current["labels"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for l in labels {
            if !existing.iter().any(|e| e.eq_ignore_ascii_case(l)) {
                existing.push(l.clone());
            }
        }
        self.put(
            &format!("issues/{number}"),
            &serde_json::json!({ "labels": existing.join(",") }),
        )
        .await
    }

    async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        let current = self.get(&format!("issues/{number}")).await?;
        let existing: Vec<String> = current["labels"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let filtered: Vec<&String> = existing
            .iter()
            .filter(|l| !l.eq_ignore_ascii_case(label))
            .collect();
        self.put(
            &format!("issues/{number}"),
            &serde_json::json!({ "labels": filtered.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",") }),
        )
        .await
    }

    async fn comment_issue(&self, number: u64, body: &str, marker: &str) -> Result<()> {
        if let Some(note_id) = self.find_comment_with_marker(number, marker).await? {
            self.put(
                &format!("issues/{number}/notes/{note_id}"),
                &serde_json::json!({ "body": body }),
            )
            .await?;
        } else {
            self.post(
                &format!("issues/{number}/notes"),
                &serde_json::json!({ "body": body }),
            )
            .await?;
        }
        Ok(())
    }

    async fn delete_comment(&self, comment_id: u64) -> Result<()> {
        // GitLab notes need the issue number — store as workaround
        // For now, attempt deletion at the project level
        self.delete(&format!("notes/{comment_id}")).await
    }

    async fn find_comment_with_marker(&self, number: u64, marker: &str) -> Result<Option<u64>> {
        let notes = self
            .get(&format!("issues/{number}/notes?per_page=100"))
            .await?;
        if let Some(arr) = notes.as_array() {
            for note in arr {
                let body = note["body"].as_str().unwrap_or("");
                if body.contains(marker) {
                    return Ok(note["id"].as_u64());
                }
            }
        }
        Ok(None)
    }

    async fn close_issue(&self, number: u64) -> Result<()> {
        self.put(
            &format!("issues/{number}"),
            &serde_json::json!({ "state_event": "close" }),
        )
        .await
    }

    async fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        let resp = self
            .post(
                "issues",
                &serde_json::json!({
                    "title": title,
                    "description": body,
                    "labels": labels.join(","),
                }),
            )
            .await?;
        Ok(resp["iid"].as_u64().context("Missing iid in response")?)
    }

    async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        // GitLab uses user IDs for assignees, not usernames — simplified version
        info!("GitLab assignee by username not directly supported, skipping for #{number}: {assignees:?}");
        Ok(())
    }

    async fn fetch_pulls(&self) -> Result<Vec<PullRequest>> {
        let items = self.get("merge_requests?state=opened&per_page=100").await?;
        let arr = items.as_array().context("Expected array")?;
        Ok(arr
            .iter()
            .map(|mr| PullRequest {
                number: mr["iid"].as_u64().unwrap_or(0),
                title: mr["title"].as_str().unwrap_or("").to_string(),
                body: mr["description"].as_str().map(String::from),
                state: "open".to_string(),
                labels: mr["labels"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                author: mr["author"]["username"].as_str().map(String::from),
                head_sha: mr["sha"].as_str().map(String::from),
                base_sha: None,
                head_ref: mr["source_branch"].as_str().map(String::from),
                base_ref: mr["target_branch"].as_str().map(String::from),
                mergeable: mr["merge_status"].as_str().map(|s| s == "can_be_merged"),
                ci_status: mr["head_pipeline"]["status"].as_str().map(String::from),
                created_at: mr["created_at"].as_str().unwrap_or("").to_string(),
                updated_at: mr["updated_at"].as_str().unwrap_or("").to_string(),
            })
            .collect())
    }

    async fn fetch_merged_pulls(&self, since: Option<&str>) -> Result<Vec<MergedPr>> {
        let mut path = "merge_requests?state=merged&per_page=100".to_string();
        if let Some(since) = since {
            path.push_str(&format!("&updated_after={since}"));
        }
        let items = self.get(&path).await?;
        let arr = items.as_array().context("Expected array")?;
        Ok(arr
            .iter()
            .map(|mr| MergedPr {
                number: mr["iid"].as_u64().unwrap_or(0),
                title: mr["title"].as_str().unwrap_or("").to_string(),
                merged_at: mr["merged_at"].as_str().unwrap_or("").to_string(),
                author: mr["author"]["username"].as_str().map(String::from),
                body: mr["description"].as_str().map(String::from),
                labels: mr["labels"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect())
    }

    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>> {
        let mr = self.get(&format!("merge_requests/{number}")).await?;
        Ok(mr["merge_status"].as_str().map(|s| s == "can_be_merged"))
    }

    async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        let url = self.api_url(&format!("merge_requests/{number}/changes"));
        let resp = self
            .http
            .get(&url)
            .header("PRIVATE-TOKEN", &self.token)
            .send()
            .await?;
        let json: serde_json::Value = resp.json().await?;
        let changes = json["changes"].as_array().context("No changes")?;
        let mut diff = String::new();
        for c in changes {
            diff.push_str(&format!(
                "--- a/{}\n+++ b/{}\n{}\n",
                c["old_path"].as_str().unwrap_or(""),
                c["new_path"].as_str().unwrap_or(""),
                c["diff"].as_str().unwrap_or(""),
            ));
        }
        Ok(diff)
    }

    async fn submit_review(
        &self,
        pr_number: u64,
        body: &str,
        comments: &[(String, u64, String)],
    ) -> Result<()> {
        // Post the summary as a note
        self.post(
            &format!("merge_requests/{pr_number}/notes"),
            &serde_json::json!({ "body": body }),
        )
        .await?;
        // Post inline comments as discussion threads
        for (path, line, comment_body) in comments {
            self.post(
                &format!("merge_requests/{pr_number}/discussions"),
                &serde_json::json!({
                    "body": comment_body,
                    "position": {
                        "base_sha": "",
                        "head_sha": "",
                        "start_sha": "",
                        "position_type": "text",
                        "new_path": path,
                        "new_line": line,
                    }
                }),
            )
            .await?;
        }
        Ok(())
    }

    async fn create_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        _draft: bool,
    ) -> Result<u64> {
        let resp = self
            .post(
                "merge_requests",
                &serde_json::json!({
                    "title": title,
                    "description": body,
                    "source_branch": head,
                    "target_branch": base,
                }),
            )
            .await?;
        Ok(resp["iid"].as_u64().context("Missing iid")?)
    }

    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        let current = self.get(&format!("merge_requests/{number}")).await?;
        let mut existing: Vec<String> = current["labels"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for l in labels {
            if !existing.iter().any(|e| e.eq_ignore_ascii_case(l)) {
                existing.push(l.clone());
            }
        }
        self.put(
            &format!("merge_requests/{number}"),
            &serde_json::json!({ "labels": existing.join(",") }),
        )
        .await
    }

    async fn comment_pr(&self, number: u64, body: &str, marker: &str) -> Result<()> {
        // Find existing note with marker
        let notes = self
            .get(&format!("merge_requests/{number}/notes?per_page=100"))
            .await?;
        if let Some(arr) = notes.as_array() {
            for note in arr {
                let note_body = note["body"].as_str().unwrap_or("");
                if note_body.contains(marker) {
                    if let Some(id) = note["id"].as_u64() {
                        self.put(
                            &format!("merge_requests/{number}/notes/{id}"),
                            &serde_json::json!({ "body": body }),
                        )
                        .await?;
                        return Ok(());
                    }
                }
            }
        }
        self.post(
            &format!("merge_requests/{number}/notes"),
            &serde_json::json!({ "body": body }),
        )
        .await?;
        Ok(())
    }

    async fn is_collaborator(&self, username: &str) -> Result<bool> {
        let members = self.get("members/all?per_page=100").await?;
        if let Some(arr) = members.as_array() {
            for m in arr {
                if m["username"].as_str() == Some(username) {
                    let level = m["access_level"].as_u64().unwrap_or(0);
                    return Ok(level >= 30); // 30 = Developer, 40 = Maintainer, 50 = Owner
                }
            }
        }
        Ok(false)
    }
}
