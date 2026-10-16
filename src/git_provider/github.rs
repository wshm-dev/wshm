use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::Config;
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;
use crate::github::Client;

use super::{Capability, CommentRef, GitProvider, MergedPr, Thread};

/// GitHub provider — a thin delegate over the octocrab-based [`Client`], so
/// GitHub behaviour (endpoints, retries, pagination, auth incl. GitHub App)
/// is exactly the historical one. Issues and PRs share GitHub's issue API
/// for labels, assignees and comments, hence the identical delegations.
pub struct GitHubProvider {
    client: Client,
}

impl GitHubProvider {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            client: Client::new(config)?,
        })
    }

    /// Access the underlying client.
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

    fn comment_marker(&self) -> &str {
        &self.client.comment_marker
    }

    fn is_authenticated(&self) -> bool {
        self.client.authenticated
    }

    fn supports(&self, cap: Capability) -> bool {
        // GitHub renders ```mermaid in issues, PRs and comments:
        // https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/creating-diagrams
        match cap {
            Capability::Mermaid
            | Capability::ReviewDecisions
            | Capability::Reactions
            | Capability::CiStatus => true,
            // The poller uses the events API instead.
            Capability::CommentPolling => false,
        }
    }

    fn as_github(&self) -> Option<&Client> {
        Some(&self.client)
    }

    async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        self.client.fetch_issues(since).await
    }

    async fn fetch_all_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        self.client.fetch_all_issues(since).await
    }

    async fn fetch_issue(&self, number: u64) -> Result<Option<Issue>> {
        self.client.fetch_issue(number).await
    }

    async fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        self.client.create_issue(title, body, labels).await
    }

    async fn close_issue(&self, number: u64) -> Result<()> {
        self.client.close_issue(number).await
    }

    async fn close_issue_not_planned(&self, number: u64) -> Result<()> {
        self.client.close_issue_not_planned(number).await
    }

    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        self.client.label_issue(number, labels).await
    }

    async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        self.client.remove_label(number, label).await
    }

    async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        self.client.add_assignees(number, assignees).await
    }

    async fn comment_issue(&self, number: u64, body: &str) -> Result<()> {
        self.client.comment_issue(number, body).await
    }

    async fn fetch_pulls(&self) -> Result<Vec<PullRequest>> {
        self.client.fetch_pulls().await
    }

    async fn fetch_pulls_incremental(
        &self,
        state: &str,
        since: Option<&str>,
    ) -> Result<Vec<PullRequest>> {
        self.client.fetch_pulls_incremental(state, since).await
    }

    async fn fetch_pull(&self, number: u64) -> Result<Option<PullRequest>> {
        self.client.fetch_pull(number).await
    }

    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>> {
        self.client.fetch_pr_mergeable(number).await
    }

    async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        self.client.fetch_pr_diff(number).await
    }

    async fn fetch_merged_pulls(&self, since: Option<&str>) -> Result<Vec<MergedPr>> {
        self.client.fetch_merged_pulls(since).await
    }

    async fn fetch_review_decisions(&self) -> Result<HashMap<u64, Option<String>>> {
        self.client.fetch_review_decisions().await
    }

    async fn fetch_pull_reactions(&self) -> Result<HashMap<u64, u32>> {
        self.client.fetch_pull_reactions().await
    }

    async fn fetch_ci_statuses(&self) -> Result<HashMap<u64, Option<String>>> {
        self.client.fetch_ci_statuses().await
    }

    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        self.client.label_pr(number, labels).await
    }

    async fn remove_pr_label(&self, number: u64, label: &str) -> Result<()> {
        self.client.remove_label(number, label).await
    }

    async fn add_pr_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        self.client.add_assignees(number, assignees).await
    }

    async fn comment_pr(&self, number: u64, body: &str) -> Result<()> {
        self.client.comment_pr(number, body).await
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

    async fn find_comment_with_marker(
        &self,
        thread: Thread,
        marker: &str,
    ) -> Result<Option<CommentRef>> {
        Ok(self
            .client
            .find_wshm_comment(thread.number(), marker)
            .await?
            .map(|id| CommentRef {
                thread,
                id,
                parent_id: None,
            }))
    }

    async fn update_comment(&self, comment: &CommentRef, body: &str) -> Result<()> {
        self.client.update_comment(comment.id, body).await
    }

    async fn delete_comment(&self, comment: &CommentRef) -> Result<()> {
        self.client.delete_comment(comment.id).await
    }

    async fn is_collaborator(&self, username: &str) -> Result<bool> {
        self.client.is_collaborator(username).await
    }
}
