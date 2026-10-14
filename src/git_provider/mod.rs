//! Forge abstraction: every pipeline, the sync layer and the daemon talk to
//! the hosting platform through [`GitProvider`], built once from config by
//! [`build_provider`]. GitHub wraps the historical octocrab [`Client`]
//! (behaviour unchanged); GitLab, Gitea/Forgejo and Azure DevOps speak their
//! own REST APIs.
//!
//! [`Client`]: crate::github::Client

pub mod azure_devops;
pub mod gitea;
pub mod github;
pub mod gitlab;
pub(crate) mod http;
#[cfg(test)]
pub(crate) mod mock;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;

/// A merged pull request with its merge date (same shape as the GitHub
/// client's type so JSON output stays identical).
pub type MergedPr = crate::github::pulls::MergedPullRequest;

/// The forge a pipeline talks to. Pipelines take `&Forge`; long-lived owners
/// (daemon state) hold a [`SharedForge`].
pub type Forge = dyn GitProvider;

/// Shared, thread-safe handle on the configured forge.
pub type SharedForge = Arc<dyn GitProvider>;

/// Optional platform features. Pipelines check these instead of matching on
/// the provider name, and a provider that lacks one must say so here rather
/// than silently pretending to succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// ```mermaid fenced blocks render as diagrams in PR/issue comments.
    Mermaid,
    /// [`GitProvider::fetch_review_decisions`] is implemented.
    ReviewDecisions,
    /// [`GitProvider::fetch_pull_reactions`] is implemented.
    Reactions,
    /// [`GitProvider::fetch_ci_statuses`] is implemented.
    CiStatus,
    /// [`GitProvider::fetch_comments_since`] is implemented (slash commands
    /// in polling mode without webhooks).
    CommentPolling,
}

/// The conversation a comment belongs to. GitHub and Gitea share one number
/// space and one comment API for issues and PRs; GitLab and Azure DevOps do
/// not, so comment operations always say which one they target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thread {
    Issue(u64),
    Pull(u64),
}

impl Thread {
    pub fn number(&self) -> u64 {
        match self {
            Thread::Issue(n) | Thread::Pull(n) => *n,
        }
    }
}

/// Handle on an existing comment, as returned by
/// [`GitProvider::find_comment_with_marker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentRef {
    pub thread: Thread,
    /// Comment / note id.
    pub id: u64,
    /// Azure DevOps PR comments live inside a thread: this is the thread id.
    pub parent_id: Option<u64>,
}

impl std::fmt::Display for CommentRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

/// A comment seen by [`GitProvider::fetch_comments_since`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolledComment {
    pub id: u64,
    pub number: u64,
    pub is_pr: bool,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// Error for an operation the platform has no equivalent for.
pub fn unsupported(provider: &str, what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what} is not supported on {provider}")
}

/// Unified trait for all git hosting providers (GitHub, GitLab, Gitea/Forgejo,
/// Azure DevOps). "Issue" and "pull request" numbers are the per-repo numbers
/// users see (GitLab `iid`, Azure work item / PR id).
#[async_trait]
pub trait GitProvider: Send + Sync {
    /// Provider name (for logging): github, gitlab, gitea, azure-devops.
    fn provider_name(&self) -> &str;

    /// Repository slug as configured (owner/repo).
    fn repo_slug(&self) -> String;

    /// Hidden HTML marker used to find wshm's own comments.
    fn comment_marker(&self) -> &str;

    /// Whether requests carry credentials. Mutating calls fail without.
    fn is_authenticated(&self) -> bool;

    /// Whether the platform supports an optional feature.
    fn supports(&self, cap: Capability) -> bool;

    /// Whether Mermaid diagrams render in PR/issue comments on this platform.
    fn supports_mermaid(&self) -> bool {
        self.supports(Capability::Mermaid)
    }

    /// Login of the account the token authenticates as, resolved once.
    /// Comments by this account are never treated as commands (loop and
    /// prompt-injection guard). `None` when unknown (GitHub relies on the
    /// marker check, unchanged).
    async fn self_username(&self) -> Option<String> {
        None
    }

    /// GitHub-only escape hatch (events API poller, GitHub App flows).
    fn as_github(&self) -> Option<&crate::github::Client> {
        None
    }

    // ── Issues ──────────────────────────────────────────────────────────

    /// Open issues, optionally only those updated since an RFC 3339 date.
    async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>>;

    /// Issues in any state, optionally only those updated since a date.
    async fn fetch_all_issues(&self, since: Option<&str>) -> Result<Vec<Issue>>;

    /// One issue from the canonical endpoint. `Ok(None)` when it does not
    /// exist (or is a pull request).
    async fn fetch_issue(&self, number: u64) -> Result<Option<Issue>>;

    /// Create a new issue. Returns its number.
    async fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<u64>;

    /// Close an issue.
    async fn close_issue(&self, number: u64) -> Result<()>;

    /// Close an issue as "not planned" (duplicates). Platforms without a
    /// close reason simply close it.
    async fn close_issue_not_planned(&self, number: u64) -> Result<()> {
        self.close_issue(number).await
    }

    /// Add labels to an issue (additive).
    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()>;

    /// Remove one label from an issue. Missing label is not an error.
    async fn remove_label(&self, number: u64, label: &str) -> Result<()>;

    /// Assign users to an issue.
    async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()>;

    /// Post or update (idempotent, by [`Self::comment_marker`]) wshm's
    /// comment on an issue.
    async fn comment_issue(&self, number: u64, body: &str) -> Result<()>;

    // ── Pull / merge requests ───────────────────────────────────────────

    /// Pull requests in every state.
    async fn fetch_pulls(&self) -> Result<Vec<PullRequest>>;

    /// Pull requests in `state` ("open", "closed", "all"), most recently
    /// updated first, stopping at those updated before `since`.
    async fn fetch_pulls_incremental(
        &self,
        state: &str,
        since: Option<&str>,
    ) -> Result<Vec<PullRequest>>;

    /// One pull request from the canonical endpoint. `Ok(None)` if absent.
    async fn fetch_pull(&self, number: u64) -> Result<Option<PullRequest>>;

    /// Mergeable status (None = not computed yet).
    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>>;

    /// Unified diff of the pull request.
    async fn fetch_pr_diff(&self, number: u64) -> Result<String>;

    /// Merged pull requests, newest first, stopping at `since`.
    async fn fetch_merged_pulls(&self, since: Option<&str>) -> Result<Vec<MergedPr>>;

    /// Review decision per open PR ("approved" / "changes_requested" /
    /// "review_required"). Requires [`Capability::ReviewDecisions`].
    async fn fetch_review_decisions(&self) -> Result<HashMap<u64, Option<String>>> {
        Err(unsupported(self.provider_name(), "review decisions"))
    }

    /// 👍 count per open PR. Requires [`Capability::Reactions`].
    async fn fetch_pull_reactions(&self) -> Result<HashMap<u64, u32>> {
        Err(unsupported(self.provider_name(), "PR reactions"))
    }

    /// CI state per open PR ("success" / "failure" / "pending"). Requires
    /// [`Capability::CiStatus`].
    async fn fetch_ci_statuses(&self) -> Result<HashMap<u64, Option<String>>> {
        Err(unsupported(self.provider_name(), "CI statuses"))
    }

    /// Add labels to a pull request (additive).
    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()>;

    /// Remove one label from a pull request.
    async fn remove_pr_label(&self, number: u64, label: &str) -> Result<()>;

    /// Assign users to a pull request.
    async fn add_pr_assignees(&self, number: u64, assignees: &[String]) -> Result<()>;

    /// Post or update (idempotent) wshm's comment on a pull request.
    async fn comment_pr(&self, number: u64, body: &str) -> Result<()>;

    /// Submit a review: a summary plus inline comments `(path, line, body)`
    /// anchored on the new side of the diff.
    async fn submit_review(
        &self,
        pr_number: u64,
        body: &str,
        comments: &[(String, u64, String)],
    ) -> Result<()>;

    /// Create a pull request. Returns its number.
    async fn create_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        draft: bool,
    ) -> Result<u64>;

    // ── Comments ────────────────────────────────────────────────────────

    /// First comment containing `marker` (or the legacy `<!-- wshm -->`).
    async fn find_comment_with_marker(
        &self,
        thread: Thread,
        marker: &str,
    ) -> Result<Option<CommentRef>>;

    /// Replace a comment's body.
    async fn update_comment(&self, comment: &CommentRef, body: &str) -> Result<()>;

    /// Delete a comment. Already-deleted is not an error.
    async fn delete_comment(&self, comment: &CommentRef) -> Result<()>;

    /// Comments created after `since` (RFC 3339) on any issue/PR of the
    /// repo. Requires [`Capability::CommentPolling`].
    async fn fetch_comments_since(&self, since: &str) -> Result<Vec<PolledComment>> {
        let _ = since;
        Err(unsupported(self.provider_name(), "comment polling"))
    }

    // ── Access ──────────────────────────────────────────────────────────

    /// Whether `username` has write access (or above) on the repository.
    /// Must return `Ok(false)` or an error when this cannot be established:
    /// it gates slash commands.
    async fn is_collaborator(&self, username: &str) -> Result<bool>;
}

/// Normalized provider id from config (`None` → "github").
pub fn provider_id(config: &crate::config::Config) -> &str {
    match config
        .git_provider
        .as_deref()
        .map(str::trim)
        .unwrap_or("github")
    {
        "" => "github",
        p => p,
    }
}

/// Build the git provider selected by `config.git_provider`.
pub fn build_provider(config: &crate::config::Config) -> Result<SharedForge> {
    build_provider_with_db(config, None)
}

/// Like [`build_provider`], with the repo DB for runtime (multi-pod)
/// overrides of provider settings stored as app settings.
pub fn build_provider_with_db(
    config: &crate::config::Config,
    db: Option<Arc<dyn crate::db::backend::DatabaseBackend>>,
) -> Result<SharedForge> {
    let base_url = config
        .git_url
        .as_deref()
        .map(|u| u.trim().trim_end_matches('/'))
        .filter(|u| !u.is_empty());

    Ok(match provider_id(config) {
        "github" => Arc::new(github::GitHubProvider::new(config)?),
        "gitlab" => Arc::new(gitlab::GitLabProvider::new(config, base_url)?),
        "gitea" => Arc::new(gitea::GiteaProvider::new(config, base_url, "gitea")?),
        "forgejo" => Arc::new(gitea::GiteaProvider::new(config, base_url, "forgejo")?),
        "azure-devops" | "azure" => Arc::new(azure_devops::AzureDevOpsProvider::new(
            config, base_url, db,
        )?),
        other => anyhow::bail!(
            "Unknown git provider: {other}. Supported: github, gitlab, gitea, forgejo, azure-devops"
        ),
    })
}

/// Resolve the API token for a non-GitHub forge: encrypted secret store
/// (per-repo → global) under `<provider>_token`, then the provider env
/// vars. Never read from config files, and never `WSHM_TOKEN` /
/// `GITHUB_TOKEN`: those are GitHub credentials and must not be sent to
/// another host.
pub fn forge_token(config: &crate::config::Config, provider: &str) -> Option<String> {
    let (key, env_names): (&str, &[&str]) = match provider {
        "gitlab" => ("gitlab_token", &["GITLAB_TOKEN"]),
        "gitea" => ("gitea_token", &["GITEA_TOKEN"]),
        "forgejo" => ("forgejo_token", &["FORGEJO_TOKEN", "GITEA_TOKEN"]),
        "azure-devops" | "azure" => (
            "azure_devops_token",
            &["AZURE_DEVOPS_TOKEN", "AZURE_DEVOPS_EXT_PAT"],
        ),
        _ => return None,
    };
    let store = crate::secrets::global();
    let slug = config.repo_slug();
    let mut found = crate::secrets::resolve(store.as_ref(), Some(&slug), key, env_names[0]);
    // A forgejo install may have stored its token as gitea_token.
    if found.is_none() && provider == "forgejo" {
        found = crate::secrets::resolve(store.as_ref(), Some(&slug), "gitea_token", "");
    }
    found
        .or_else(|| env_names[1..].iter().find_map(|n| std::env::var(n).ok()))
        .filter(|t| !t.trim().is_empty())
}

/// Secret-store keys holding forge credentials; a change to any of them
/// must rebuild the daemon's provider.
pub const FORGE_AUTH_KEYS: [&str; 8] = [
    "github_token",
    "github_app_id",
    "github_app_installation_id",
    "github_app_private_key",
    "gitlab_token",
    "gitea_token",
    "forgejo_token",
    "azure_devops_token",
];

/// Build a public web URL for an issue on the configured forge.
///
/// Used by the daemon's REST API to surface a "View on …" link from
/// the SPA modals. Returns `None` for unknown providers; callers
/// fall back to hiding the link rather than guessing.
pub fn web_url_for_issue(config: &crate::config::Config, number: u64) -> Option<String> {
    let owner = &config.repo_owner;
    let repo = &config.repo_name;
    let provider = config.git_provider.as_deref().unwrap_or("github");
    let base = forge_base_url(config, provider);
    match provider {
        "github" => Some(format!("{base}/{owner}/{repo}/issues/{number}")),
        "gitlab" => Some(format!("{base}/{owner}/{repo}/-/issues/{number}")),
        "gitea" | "forgejo" => Some(format!("{base}/{owner}/{repo}/issues/{number}")),
        "azure-devops" | "azure" => {
            // Azure DevOps "issues" are work items, identified by ID across the whole org.
            // `repo_owner` here is the org, `repo_name` carries `project/repo`.
            let project = repo.split('/').next().unwrap_or(repo);
            Some(format!("{base}/{owner}/{project}/_workitems/edit/{number}"))
        }
        _ => None,
    }
}

/// Build a public web URL for a pull/merge request on the configured forge.
pub fn web_url_for_pr(config: &crate::config::Config, number: u64) -> Option<String> {
    let owner = &config.repo_owner;
    let repo = &config.repo_name;
    let provider = config.git_provider.as_deref().unwrap_or("github");
    let base = forge_base_url(config, provider);
    match provider {
        "github" => Some(format!("{base}/{owner}/{repo}/pull/{number}")),
        "gitlab" => Some(format!("{base}/{owner}/{repo}/-/merge_requests/{number}")),
        "gitea" | "forgejo" => Some(format!("{base}/{owner}/{repo}/pulls/{number}")),
        "azure-devops" | "azure" => {
            let parts: Vec<&str> = repo.splitn(2, '/').collect();
            let project = parts.first().copied().unwrap_or(repo.as_str());
            let repo_only = parts.get(1).copied().unwrap_or(repo.as_str());
            Some(format!(
                "{base}/{owner}/{project}/_git/{repo_only}/pullrequest/{number}"
            ))
        }
        _ => None,
    }
}

/// Resolve the public base URL for the configured forge, with sensible
/// defaults for the SaaS instance of each provider.
fn forge_base_url(config: &crate::config::Config, provider: &str) -> String {
    if let Some(custom) = config.git_url.as_deref() {
        let trimmed = custom.trim_end_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    match provider {
        "github" => "https://github.com",
        "gitlab" => "https://gitlab.com",
        "gitea" => "https://gitea.com",
        "forgejo" => "https://codeberg.org",
        "azure-devops" | "azure" => "https://dev.azure.com",
        _ => "",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: Option<&str>, slug: (&str, &str)) -> crate::config::Config {
        crate::config::Config {
            git_provider: provider.map(String::from),
            repo_owner: slug.0.into(),
            repo_name: slug.1.into(),
            ..Default::default()
        }
    }

    #[test]
    fn build_provider_routes_on_git_provider() {
        for (id, name, mermaid) in [
            ("gitlab", "gitlab", true),
            ("gitea", "gitea", true),
            ("forgejo", "forgejo", true),
            ("azure-devops", "azure-devops", false),
            ("azure", "azure-devops", false),
        ] {
            let p = build_provider(&config(Some(id), ("acme", "proj/repo"))).unwrap();
            assert_eq!(p.provider_name(), name);
            assert_eq!(p.supports_mermaid(), mermaid, "{id}");
            assert!(p.as_github().is_none());
        }
        assert!(build_provider(&config(Some("svn"), ("a", "b"))).is_err());
    }

    #[tokio::test]
    async fn github_is_the_default_and_keeps_its_client() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let p = build_provider(&config(None, ("rtk-ai", "rtk"))).unwrap();
        assert_eq!(p.provider_name(), "github");
        assert!(p.supports_mermaid());
        assert!(p.supports(Capability::ReviewDecisions));
        assert!(p.as_github().is_some());
        assert_eq!(p.repo_slug(), "rtk-ai/rtk");
    }
}
