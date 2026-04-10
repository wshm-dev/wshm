use anyhow::Result;
use async_trait::async_trait;

use crate::config::Config;
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;
use crate::github::Client;

use super::{GitProvider, MergedPr};

/// GitHub provider — wraps the existing octocrab-based Client.
/// Supports both personal repos and organization repos.
pub struct GitHubProvider {
    client: Client,
}

impl GitHubProvider {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            client: Client::new(config)?,
        })
    }

    /// Access the underlying client for backward compatibility.
    pub fn client(&self) -> &Client {
        &self.client
    }
}

#[async_trait]
impl GitProvider for GitHubProvider {
    fn provider_name(&self) -> &str {
        "github"
    }

    fn repo_slug(&self) -> String {
        format!("{}/{}", self.client.owner, self.client.repo)
    }

    async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        self.client.fetch_issues(since).await
    }

    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        self.client.label_issue(number, labels).await
    }

    async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        self.client.remove_label(number, label).await
    }

    async fn comment_issue(&self, number: u64, body: &str, _marker: &str) -> Result<()> {
        // GitHub client handles marker internally via ensure_wshm_marker + find_wshm_comment
        self.client.comment_issue(number, body).await
    }

    async fn delete_comment(&self, comment_id: u64) -> Result<()> {
        self.client.delete_comment(comment_id).await
    }

    async fn find_comment_with_marker(&self, number: u64, marker: &str) -> Result<Option<u64>> {
        self.client.find_wshm_comment(number, marker).await
    }

    async fn close_issue(&self, number: u64) -> Result<()> {
        self.client.close_issue(number).await
    }

    async fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        self.client.create_issue(title, body, labels).await
    }

    async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        self.client.add_assignees(number, assignees).await
    }

    async fn fetch_pulls(&self) -> Result<Vec<PullRequest>> {
        self.client.fetch_pulls().await
    }

    async fn fetch_merged_pulls(&self, since: Option<&str>) -> Result<Vec<MergedPr>> {
        let pulls = self.client.fetch_merged_pulls(since).await?;
        Ok(pulls
            .into_iter()
            .map(|p| MergedPr {
                number: p.number,
                title: p.title,
                merged_at: p.merged_at,
                author: p.author,
                body: p.body,
                labels: p.labels,
            })
            .collect())
    }

    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>> {
        self.client.fetch_pr_mergeable(number).await
    }

    async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        self.client.fetch_pr_diff(number).await
    }

    async fn submit_review(
        &self,
        pr_number: u64,
        body: &str,
        comments: &[(String, u64, String)],
    ) -> Result<()> {
        self.client.submit_review(pr_number, body, comments).await
    }

    async fn create_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        draft: bool,
    ) -> Result<u64> {
        if draft {
            self.client.create_draft_pr(title, body, head, base).await
        } else {
            self.client.create_pr(title, body, head, base).await
        }
    }

    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        self.client.label_pr(number, labels).await
    }

    async fn comment_pr(&self, number: u64, body: &str, _marker: &str) -> Result<()> {
        self.client.comment_pr(number, body).await
    }

    async fn is_collaborator(&self, username: &str) -> Result<bool> {
        self.client.is_collaborator(username).await
    }
}
