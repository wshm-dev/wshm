use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::Cli;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub github: GitHubConfig,

    #[serde(default)]
    pub ai: AiConfig,

    #[serde(default)]
    pub triage: TriageConfig,

    #[serde(default)]
    pub pr: PrConfig,

    #[serde(default)]
    pub queue: QueueConfig,

    #[serde(default)]
    pub conflicts: ConflictConfig,

    #[serde(default)]
    pub sync: SyncConfig,

    /// Resolved at runtime, not from config file
    #[serde(skip)]
    pub repo_owner: String,

    /// Resolved at runtime, not from config file
    #[serde(skip)]
    pub repo_name: String,

    /// Resolved at runtime
    #[serde(skip)]
    pub wshm_dir: PathBuf,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct GitHubConfig {}

#[derive(Debug, Deserialize, Serialize)]
pub struct AiConfig {
    #[serde(default = "default_ai_provider")]
    pub provider: String,

    #[serde(default = "default_ai_model")]
    pub model: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: default_ai_provider(),
            model: default_ai_model(),
        }
    }
}

fn default_ai_provider() -> String {
    "anthropic".to_string()
}

fn default_ai_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TriageConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub auto_fix: bool,

    #[serde(default = "default_confidence")]
    pub auto_fix_confidence: f64,

    #[serde(default = "default_label_bug")]
    pub labels_bug: String,

    #[serde(default = "default_label_feature")]
    pub labels_feature: String,

    #[serde(default = "default_label_duplicate")]
    pub labels_duplicate: String,

    #[serde(default = "default_label_wontfix")]
    pub labels_wontfix: String,

    #[serde(default = "default_label_needs_info")]
    pub labels_needs_info: String,
}

impl Default for TriageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_fix: false,
            auto_fix_confidence: default_confidence(),
            labels_bug: default_label_bug(),
            labels_feature: default_label_feature(),
            labels_duplicate: default_label_duplicate(),
            labels_wontfix: default_label_wontfix(),
            labels_needs_info: default_label_needs_info(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_confidence() -> f64 {
    0.85
}
fn default_label_bug() -> String {
    "bug".to_string()
}
fn default_label_feature() -> String {
    "feature".to_string()
}
fn default_label_duplicate() -> String {
    "duplicate".to_string()
}
fn default_label_wontfix() -> String {
    "wontfix".to_string()
}
fn default_label_needs_info() -> String {
    "needs-info".to_string()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PrConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_true")]
    pub auto_label: bool,

    #[serde(default = "default_true")]
    pub risk_labels: bool,
}

impl Default for PrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_label: true,
            risk_labels: true,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct QueueConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_merge_threshold")]
    pub merge_threshold: i32,

    #[serde(default = "default_merge_strategy")]
    pub strategy: String,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            merge_threshold: default_merge_threshold(),
            strategy: default_merge_strategy(),
        }
    }
}

fn default_merge_threshold() -> i32 {
    15
}
fn default_merge_strategy() -> String {
    "rebase".to_string()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ConflictConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub auto_resolve: bool,

    #[serde(default = "default_confidence")]
    pub auto_resolve_confidence: f64,
}

impl Default for ConflictConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_resolve: false,
            auto_resolve_confidence: default_confidence(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SyncConfig {
    #[serde(default = "default_sync_interval")]
    pub interval_minutes: u32,

    #[serde(default = "default_full_sync_interval")]
    pub full_sync_interval_hours: u32,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            interval_minutes: default_sync_interval(),
            full_sync_interval_hours: default_full_sync_interval(),
        }
    }
}

fn default_sync_interval() -> u32 {
    5
}
fn default_full_sync_interval() -> u32 {
    24
}

impl Config {
    pub fn load(cli: &Cli) -> Result<Self> {
        let wshm_dir = PathBuf::from(".wshm");
        let config_path = wshm_dir.join("config.toml");

        let mut config: Config = if config_path.exists() {
            let content =
                fs::read_to_string(&config_path).context("Failed to read .wshm/config.toml")?;
            toml::from_str(&content).context("Failed to parse .wshm/config.toml")?
        } else {
            Config::default()
        };

        config.wshm_dir = wshm_dir;

        // Resolve repo owner/name from CLI flag or git remote
        if let Some(ref repo) = cli.repo {
            let parts: Vec<&str> = repo.splitn(2, '/').collect();
            if parts.len() != 2 {
                anyhow::bail!("--repo must be in format owner/repo");
            }
            config.repo_owner = parts[0].to_string();
            config.repo_name = parts[1].to_string();
        } else {
            let (owner, name) = detect_repo()?;
            config.repo_owner = owner;
            config.repo_name = name;
        }

        Ok(config)
    }

    pub fn init_template() -> Result<()> {
        let wshm_dir = Path::new(".wshm");
        fs::create_dir_all(wshm_dir)?;

        let config_path = wshm_dir.join("config.toml");
        if config_path.exists() {
            anyhow::bail!(".wshm/config.toml already exists");
        }

        let template = r#"[github]
# Token from env var GITHUB_TOKEN or WSHM_TOKEN (never stored in config)

[ai]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
# API key from env var ANTHROPIC_API_KEY (never stored in config)

[triage]
enabled = true
auto_fix = false
auto_fix_confidence = 0.85
labels_bug = "bug"
labels_feature = "feature"
labels_duplicate = "duplicate"
labels_wontfix = "wontfix"
labels_needs_info = "needs-info"

[pr]
enabled = true
auto_label = true
risk_labels = true

[queue]
enabled = true
merge_threshold = 15
strategy = "rebase"

[conflicts]
enabled = true
auto_resolve = false
auto_resolve_confidence = 0.85

[sync]
interval_minutes = 5
full_sync_interval_hours = 24
"#;

        fs::write(&config_path, template)?;
        Ok(())
    }

    pub fn github_token(&self) -> Result<String> {
        std::env::var("WSHM_TOKEN")
            .or_else(|_| std::env::var("GITHUB_TOKEN"))
            .or_else(|_| gh_auth_token())
            .context("No GitHub token found. Set GITHUB_TOKEN, WSHM_TOKEN, or authenticate with `gh auth login`")
    }

    pub fn ai_api_key(&self) -> Result<String> {
        match self.ai.provider.as_str() {
            "anthropic" => std::env::var("ANTHROPIC_API_KEY")
                .context("Set ANTHROPIC_API_KEY environment variable"),
            "openai" => {
                std::env::var("OPENAI_API_KEY").context("Set OPENAI_API_KEY environment variable")
            }
            other => anyhow::bail!("Unknown AI provider: {other}"),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            github: GitHubConfig::default(),
            ai: AiConfig::default(),
            triage: TriageConfig::default(),
            pr: PrConfig::default(),
            queue: QueueConfig::default(),
            conflicts: ConflictConfig::default(),
            sync: SyncConfig::default(),
            repo_owner: String::new(),
            repo_name: String::new(),
            wshm_dir: PathBuf::from(".wshm"),
        }
    }
}

fn gh_auth_token() -> Result<String, std::env::VarError> {
    std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if token.is_empty() {
                    None
                } else {
                    Some(token)
                }
            } else {
                None
            }
        })
        .ok_or(std::env::VarError::NotPresent)
}

fn detect_repo() -> Result<(String, String)> {
    let repo = git2::Repository::open_from_env()
        .context("Not inside a git repository. Use --repo owner/repo")?;

    let remote = repo
        .find_remote("origin")
        .context("No 'origin' remote found. Use --repo owner/repo")?;

    let url = remote.url().context("Remote URL is not valid UTF-8")?;
    parse_github_url(url)
}

fn parse_github_url(url: &str) -> Result<(String, String)> {
    // Handle SSH: git@github.com:owner/repo.git
    if let Some(path) = url.strip_prefix("git@github.com:") {
        let path = path.strip_suffix(".git").unwrap_or(path);
        let parts: Vec<&str> = path.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // Handle HTTPS: https://github.com/owner/repo.git
    if let Some(path) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        let path = path.strip_suffix(".git").unwrap_or(path);
        let parts: Vec<&str> = path.splitn(2, '/').collect();
        if parts.len() == 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    anyhow::bail!("Cannot parse GitHub owner/repo from remote URL: {url}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssh_url() {
        let (owner, repo) = parse_github_url("git@github.com:user/project.git").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "project");
    }

    #[test]
    fn test_parse_https_url() {
        let (owner, repo) = parse_github_url("https://github.com/user/project.git").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "project");
    }

    #[test]
    fn test_parse_https_no_git_suffix() {
        let (owner, repo) = parse_github_url("https://github.com/user/project").unwrap();
        assert_eq!(owner, "user");
        assert_eq!(repo, "project");
    }
}
