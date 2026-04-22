pub mod azure_devops;
pub mod gitea;
pub mod github;
pub mod gitlab;

use anyhow::Result;
use async_trait::async_trait;

use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;

/// A merged pull request with its merge date.
#[derive(Debug, Clone)]
pub struct MergedPr {
    pub number: u64,
    pub title: String,
    pub merged_at: String,
    pub author: Option<String>,
    pub body: Option<String>,
    pub labels: Vec<String>,
}

/// Inline review comment for PR review submissions.
pub struct ReviewComment {
    pub path: String,
    pub line: u64,
    pub body: String,
}

/// Unified trait for all git hosting providers (GitHub, GitLab, Gitea, Azure DevOps).
/// Each provider implements this trait to allow wshm to work with any git platform.
#[async_trait]
pub trait GitProvider: Send + Sync {
    /// Provider name (for logging).
    fn provider_name(&self) -> &str;

    /// Repository slug (owner/repo).
    fn repo_slug(&self) -> String;

    // ── Issues ──────────────────────────────────────────────

    /// Fetch open issues, optionally since a timestamp.
    async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>>;

    /// Apply labels to an issue or PR.
    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()>;

    /// Remove a label from an issue or PR.
    async fn remove_label(&self, number: u64, label: &str) -> Result<()>;

    /// Post or update an idempotent comment on an issue/PR.
    /// Uses the marker to find and update existing comments.
    async fn comment_issue(&self, number: u64, body: &str, marker: &str) -> Result<()>;

    /// Delete a comment by ID.
    async fn delete_comment(&self, comment_id: u64) -> Result<()>;

    /// Find a comment containing the given marker. Returns comment ID if found.
    async fn find_comment_with_marker(&self, number: u64, marker: &str) -> Result<Option<u64>>;

    /// Close an issue.
    async fn close_issue(&self, number: u64) -> Result<()>;

    /// Create a new issue. Returns issue number.
    async fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<u64>;

    /// Add assignees to an issue/PR.
    async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()>;

    // ── Pull Requests ───────────────────────────────────────

    /// Fetch open pull requests.
    async fn fetch_pulls(&self) -> Result<Vec<PullRequest>>;

    /// Fetch merged pull requests since a date.
    async fn fetch_merged_pulls(&self, since: Option<&str>) -> Result<Vec<MergedPr>>;

    /// Fetch mergeable status for a PR.
    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>>;

    /// Fetch the unified diff for a PR.
    async fn fetch_pr_diff(&self, number: u64) -> Result<String>;

    /// Submit a PR review with inline comments.
    async fn submit_review(
        &self,
        pr_number: u64,
        body: &str,
        comments: &[(String, u64, String)],
    ) -> Result<()>;

    /// Create a pull request. Returns PR number.
    async fn create_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        draft: bool,
    ) -> Result<u64>;

    /// Label a PR (same API as label_issue on most platforms).
    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()>;

    /// Post or update a comment on a PR.
    async fn comment_pr(&self, number: u64, body: &str, marker: &str) -> Result<()>;

    // ── Auth ────────────────────────────────────────────────

    /// Check if a user is a collaborator (write access or above).
    async fn is_collaborator(&self, username: &str) -> Result<bool>;
}

/// Build a git provider from config.
pub fn build_provider(config: &crate::config::Config) -> Result<Box<dyn GitProvider>> {
    let provider = config.git_provider.as_deref().unwrap_or("github");
    let base_url = config.git_url.as_deref();

    match provider {
        "github" => Ok(Box::new(github::GitHubProvider::new(config)?)),
        "gitlab" => Ok(Box::new(gitlab::GitLabProvider::new(config, base_url)?)),
        "gitea" => Ok(Box::new(gitea::GiteaProvider::new(config, base_url)?)),
        "azure-devops" | "azure" => Ok(Box::new(azure_devops::AzureDevOpsProvider::new(
            config, base_url,
        )?)),
        _ => anyhow::bail!(
            "Unknown git provider: {provider}. Supported: github, gitlab, gitea, azure-devops"
        ),
    }
}
