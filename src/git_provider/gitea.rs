use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::info;

use crate::config::Config;
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;

use super::{GitProvider, MergedPr};

/// Gitea provider — uses Gitea REST API v1.
/// Compatible with Gitea, Forgejo, and Codeberg.
pub struct GiteaProvider {
    http: reqwest::Client,
    base_url: String,
    owner: String,
    repo: String,
    token: String,
}

impl GiteaProvider {
    pub fn new(config: &Config, base_url: Option<&str>) -> Result<Self> {
        let token = std::env::var("GITEA_TOKEN")
            .or_else(|_| std::env::var("WSHM_TOKEN"))
            .context("No Gitea token found. Set GITEA_TOKEN or WSHM_TOKEN.")?;

        let base = base_url.unwrap_or("https://gitea.com");

        let http = reqwest::Client::builder()
            .user_agent("wshm")
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        info!(
            "Gitea provider initialized: {base} for {}/{}",
            config.repo_owner, config.repo_name
        );

        Ok(Self {
            http,
            base_url: base.trim_end_matches('/').to_string(),
            owner: config.repo_owner.clone(),
            repo: config.repo_name.clone(),
            token,
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!(
            "{}/api/v1/repos/{}/{}/{}",
            self.base_url, self.owner, self.repo, path
        )
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let url = self.api_url(path);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await
            .with_context(|| format!("Gitea GET {path}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!(
                "Gitea API error ({status}): {}",
                &text[..text.len().min(200)]
            );
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn post_json(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = self.api_url(path);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!(
                "Gitea API error ({status}): {}",
                &text[..text.len().min(200)]
            );
        }
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }

    async fn patch(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = self.api_url(path);
        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("token {}", self.token))
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!(
                "Gitea API error ({status}): {}",
                &text[..text.len().min(200)]
            );
        }
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }

    async fn delete_req(&self, path: &str) -> Result<()> {
        let url = self.api_url(path);
        let resp = self
            .http
            .delete(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await?;
        if !resp.status().is_success() && resp.status().as_u16() != 404 {
            let text = resp.text().await?;
            anyhow::bail!("Gitea DELETE error: {}", &text[..text.len().min(200)]);
        }
        Ok(())
    }
}

#[async_trait]
impl GitProvider for GiteaProvider {
    fn provider_name(&self) -> &str {
        "gitea"
    }
    fn repo_slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        let mut path = "issues?state=open&type=issues&limit=50".to_string();
        if let Some(s) = since {
            path.push_str(&format!("&since={s}"));
        }
        let items = self.get(&path).await?;
        let arr = items.as_array().context("Expected array")?;
        Ok(arr
            .iter()
            .map(|i| Issue {
                number: i["number"].as_u64().unwrap_or(0),
                title: i["title"].as_str().unwrap_or("").to_string(),
                body: i["body"].as_str().map(String::from),
                state: "open".to_string(),
                labels: i["labels"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v["name"].as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                author: i["user"]["login"].as_str().map(String::from),
                created_at: i["created_at"].as_str().unwrap_or("").to_string(),
                updated_at: i["updated_at"].as_str().unwrap_or("").to_string(),
                reactions_plus1: 0,
                reactions_total: 0,
            })
            .collect())
    }

    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        // Gitea: POST /repos/{owner}/{repo}/issues/{index}/labels
        // Need label IDs, so first get or create labels
        for label in labels {
            self.post_json(
                &format!("issues/{number}/labels"),
                &serde_json::json!({ "labels": [label] }),
            )
            .await
            .ok();
        }
        Ok(())
    }

    async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        // Gitea needs label ID to remove — simplified: get label ID first
        let labels = self.get("labels").await?;
        if let Some(arr) = labels.as_array() {
            for l in arr {
                if l["name"].as_str() == Some(label) {
                    if let Some(id) = l["id"].as_u64() {
                        self.delete_req(&format!("issues/{number}/labels/{id}"))
                            .await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn comment_issue(&self, number: u64, body: &str, marker: &str) -> Result<()> {
        if let Some(id) = self.find_comment_with_marker(number, marker).await? {
            self.patch(
                &format!("issues/comments/{id}"),
                &serde_json::json!({ "body": body }),
            )
            .await?;
        } else {
            self.post_json(
                &format!("issues/{number}/comments"),
                &serde_json::json!({ "body": body }),
            )
            .await?;
        }
        Ok(())
    }

    async fn delete_comment(&self, comment_id: u64) -> Result<()> {
        self.delete_req(&format!("issues/comments/{comment_id}"))
            .await
    }

    async fn find_comment_with_marker(&self, number: u64, marker: &str) -> Result<Option<u64>> {
        let comments = self.get(&format!("issues/{number}/comments")).await?;
        if let Some(arr) = comments.as_array() {
            for c in arr {
                if c["body"].as_str().unwrap_or("").contains(marker) {
                    return Ok(c["id"].as_u64());
                }
            }
        }
        Ok(None)
    }

    async fn close_issue(&self, number: u64) -> Result<()> {
        self.patch(
            &format!("issues/{number}"),
            &serde_json::json!({ "state": "closed" }),
        )
        .await?;
        Ok(())
    }

    async fn create_issue(&self, title: &str, body: &str, _labels: &[String]) -> Result<u64> {
        let resp = self
            .post_json(
                "issues",
                &serde_json::json!({ "title": title, "body": body }),
            )
            .await?;
        Ok(resp["number"].as_u64().context("Missing number")?)
    }

    async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        self.patch(
            &format!("issues/{number}"),
            &serde_json::json!({ "assignees": assignees }),
        )
        .await?;
        Ok(())
    }

    async fn fetch_pulls(&self) -> Result<Vec<PullRequest>> {
        let items = self.get("pulls?state=open&limit=50").await?;
        let arr = items.as_array().context("Expected array")?;
        Ok(arr
            .iter()
            .map(|pr| PullRequest {
                number: pr["number"].as_u64().unwrap_or(0),
                title: pr["title"].as_str().unwrap_or("").to_string(),
                body: pr["body"].as_str().map(String::from),
                state: "open".to_string(),
                labels: pr["labels"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v["name"].as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                author: pr["user"]["login"].as_str().map(String::from),
                head_sha: pr["head"]["sha"].as_str().map(String::from),
                base_sha: pr["base"]["sha"].as_str().map(String::from),
                head_ref: pr["head"]["ref"].as_str().map(String::from),
                base_ref: pr["base"]["ref"].as_str().map(String::from),
                mergeable: pr["mergeable"].as_bool(),
                ci_status: None,
                created_at: pr["created_at"].as_str().unwrap_or("").to_string(),
                updated_at: pr["updated_at"].as_str().unwrap_or("").to_string(),
            })
            .collect())
    }

    async fn fetch_merged_pulls(&self, _since: Option<&str>) -> Result<Vec<MergedPr>> {
        let items = self.get("pulls?state=closed&limit=50").await?;
        let arr = items.as_array().context("Expected array")?;
        Ok(arr
            .iter()
            .filter(|pr| pr["merged"].as_bool() == Some(true))
            .map(|pr| MergedPr {
                number: pr["number"].as_u64().unwrap_or(0),
                title: pr["title"].as_str().unwrap_or("").to_string(),
                merged_at: pr["merged_at"].as_str().unwrap_or("").to_string(),
                author: pr["user"]["login"].as_str().map(String::from),
                body: pr["body"].as_str().map(String::from),
                labels: Vec::new(),
            })
            .collect())
    }

    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>> {
        let pr = self.get(&format!("pulls/{number}")).await?;
        Ok(pr["mergeable"].as_bool())
    }

    async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        let url = format!(
            "{}/api/v1/repos/{}/{}/pulls/{number}.diff",
            self.base_url, self.owner, self.repo
        );
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await?;
        Ok(resp.text().await?)
    }

    async fn submit_review(
        &self,
        pr_number: u64,
        body: &str,
        _comments: &[(String, u64, String)],
    ) -> Result<()> {
        self.post_json(
            &format!("pulls/{pr_number}/reviews"),
            &serde_json::json!({ "body": body, "event": "COMMENT" }),
        )
        .await?;
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
            .post_json(
                "pulls",
                &serde_json::json!({ "title": title, "body": body, "head": head, "base": base }),
            )
            .await?;
        Ok(resp["number"].as_u64().context("Missing number")?)
    }

    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        self.label_issue(number, labels).await
    }

    async fn comment_pr(&self, number: u64, body: &str, marker: &str) -> Result<()> {
        self.comment_issue(number, body, marker).await
    }

    async fn is_collaborator(&self, username: &str) -> Result<bool> {
        let url = self.api_url(&format!("collaborators/{username}"));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("token {}", self.token))
            .send()
            .await?;
        Ok(resp.status().is_success())
    }
}
