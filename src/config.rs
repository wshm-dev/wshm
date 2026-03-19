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

    #[serde(default)]
    pub fix: FixConfig,

    #[serde(default)]
    pub daemon: DaemonConfig,

    #[serde(default)]
    pub branding: BrandingConfig,

    #[serde(default)]
    pub update: UpdateConfig,

    #[serde(default)]
    pub export: ExportConfig,

    #[serde(default)]
    pub vault: Option<VaultConfig>,

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

    /// Optional base URL override (for custom endpoints, proxies, etc.)
    #[serde(default)]
    pub base_url: Option<String>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: default_ai_provider(),
            model: default_ai_model(),
            base_url: None,
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

    /// Override AI model for triage (uses [ai].model if not set)
    #[serde(default)]
    pub model: Option<String>,

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

    /// Re-triage interval in hours (0 = disabled). Re-evaluates previously triaged issues.
    #[serde(default)]
    pub retriage_interval_hours: u32,
}

impl Default for TriageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_fix: false,
            auto_fix_confidence: default_confidence(),
            model: None,
            labels_bug: default_label_bug(),
            labels_feature: default_label_feature(),
            labels_duplicate: default_label_duplicate(),
            labels_wontfix: default_label_wontfix(),
            labels_needs_info: default_label_needs_info(),
            retriage_interval_hours: 0,
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

    /// Override AI model for PR analysis (uses [ai].model if not set)
    #[serde(default)]
    pub model: Option<String>,
}

impl Default for PrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_label: true,
            risk_labels: true,
            model: None,
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

#[derive(Debug, Deserialize, Serialize)]
pub struct FixConfig {
    /// AI tool to use: claude-code, codex (default: claude-code)
    #[serde(default = "default_fix_tool")]
    pub tool: String,

    /// Docker image for sandboxed execution
    #[serde(default = "default_fix_image")]
    pub docker_image: String,

    /// Extra env var names to forward into Docker containers
    #[serde(default)]
    pub secret_env: Vec<String>,

    /// Only auto-fix issues from trusted authors (collaborators with write+)
    #[serde(default = "default_true")]
    pub trusted_authors_only: bool,

    /// Scan generated diff for suspicious patterns before committing
    #[serde(default = "default_true")]
    pub scan_diff: bool,

    /// Create PRs as draft (require manual review before merge)
    #[serde(default = "default_true")]
    pub draft_pr: bool,

    /// Base branch to create fix branches from (default: "main")
    #[serde(default = "default_base_branch")]
    pub base_branch: String,

    /// Override AI model for auto-fix (uses [ai].model if not set)
    #[serde(default)]
    pub model: Option<String>,

    /// Explicit whitelist of GitHub usernames allowed to trigger auto-fix.
    /// If non-empty, only these users can trigger auto-fix (overrides trusted_authors_only).
    /// If empty, falls back to trusted_authors_only (collaborator check via API).
    #[serde(default)]
    pub allowed_users: Vec<String>,

    /// Command to run tests after code generation (e.g. "cargo test", "bun test").
    /// If set and tests fail, the fix is aborted and a comment is posted on the issue.
    #[serde(default)]
    pub test_command: Option<String>,

    /// Maximum retries: if tests fail, re-prompt the AI with the error output (default: 0)
    #[serde(default)]
    pub test_retries: u32,
}

impl Default for FixConfig {
    fn default() -> Self {
        Self {
            tool: default_fix_tool(),
            docker_image: default_fix_image(),
            secret_env: Vec::new(),
            trusted_authors_only: true,
            scan_diff: true,
            draft_pr: true,
            base_branch: default_base_branch(),
            model: None,
            allowed_users: Vec::new(),
            test_command: None,
            test_retries: 0,
        }
    }
}

fn default_base_branch() -> String {
    "main".to_string()
}

fn default_fix_tool() -> String {
    "claude-code".to_string()
}
fn default_fix_image() -> String {
    "wshm-sandbox:latest".to_string()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DaemonConfig {
    #[serde(default = "default_daemon_bind")]
    pub bind: String,

    #[serde(default)]
    pub webhook_secret: Option<String>,

    #[serde(default)]
    pub apply: bool,

    #[serde(default)]
    pub icm_enabled: bool,

    #[serde(default = "default_icm_prefix")]
    pub icm_topic_prefix: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bind: default_daemon_bind(),
            webhook_secret: None,
            apply: false,
            icm_enabled: false,
            icm_topic_prefix: default_icm_prefix(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BrandingConfig {
    /// Bot display name in comments (default: "wshm")
    #[serde(default = "default_bot_name")]
    pub name: String,

    /// Bot URL linked in footers (default: "https://github.com/pszymkowiak/wshm")
    #[serde(default = "default_bot_url")]
    pub url: String,

    /// Avatar/logo URL shown in comment headers (optional, Markdown image)
    #[serde(default)]
    pub avatar_url: Option<String>,

    /// Tagline shown in comment headers (optional)
    #[serde(default)]
    pub tagline: Option<String>,

    /// Slash command prefix in issue comments (default: "/wshm")
    #[serde(default = "default_command_prefix")]
    pub command_prefix: String,

    /// Footer template. Placeholders: {name}, {url}
    /// Default: "*{action} by [{name}]({url})*"
    #[serde(default)]
    pub footer_template: Option<String>,
}

impl Default for BrandingConfig {
    fn default() -> Self {
        Self {
            name: default_bot_name(),
            url: default_bot_url(),
            avatar_url: None,
            tagline: None,
            command_prefix: default_command_prefix(),
            footer_template: None,
        }
    }
}

impl BrandingConfig {
    /// Hidden HTML marker for idempotent comment updates.
    pub fn comment_marker(&self) -> String {
        format!("<!-- {} -->", self.name)
    }

    /// Build the footer line for a comment. `action` is e.g. "Triaged", "Analyzed", "Reviewed".
    pub fn footer(&self, action: &str) -> String {
        let tmpl = self.footer_template.as_deref()
            .unwrap_or("🤖 *{action} automatically by [{name}]({url})* · This is an automated analysis, not a human review.");

        let result = tmpl
            .replace("{action}", action)
            .replace("{name}", &self.name)
            .replace("{url}", &self.url);

        format!("---\n{}\n{}", result, self.comment_marker())
    }

    /// Build a comment header — always shows a clear bot banner.
    pub fn header(&self) -> String {
        let icon = if let Some(ref avatar) = self.avatar_url {
            // Only allow HTTPS URLs to prevent javascript: or data: URI injection
            if avatar.starts_with("https://") && !avatar.contains('"') && !avatar.contains('>') {
                format!("<img src=\"{avatar}\" width=\"24\" height=\"24\" align=\"absmiddle\">")
            } else {
                tracing::warn!("Ignoring invalid avatar_url (must be HTTPS, no special chars)");
                "🧞".to_string()
            }
        } else {
            "🧞".to_string()
        };

        let tagline = self.tagline.as_deref().unwrap_or("Automated triage by AI");

        format!("> {icon} **{name}** · {tagline}\n\n", name = self.name,)
    }
}

fn default_bot_name() -> String {
    "wshm".to_string()
}
fn default_bot_url() -> String {
    "https://github.com/pszymkowiak/wshm".to_string()
}
fn default_command_prefix() -> String {
    "/wshm".to_string()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateConfig {
    /// Enable automatic update checks in daemon mode (default: false)
    #[serde(default)]
    pub enabled: bool,

    /// Check interval in hours (default: 6)
    #[serde(default = "default_update_interval")]
    pub interval_hours: u32,

    /// Auto-apply updates without confirmation (default: false, daemon sets true)
    #[serde(default)]
    pub auto_apply: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_hours: default_update_interval(),
            auto_apply: false,
        }
    }
}

fn default_update_interval() -> u32 {
    6
}

// ── Export config ──────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ExportConfig {
    #[serde(default)]
    pub storage: Option<StorageConfig>,

    #[serde(default)]
    pub database: Option<DatabaseExportConfig>,

    #[serde(default)]
    pub webhooks: Vec<WebhookConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StorageConfig {
    pub provider: String,

    #[serde(default)]
    pub bucket: Option<String>,

    #[serde(default)]
    pub prefix: Option<String>,

    #[serde(default)]
    pub region: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DatabaseExportConfig {
    pub provider: String,

    #[serde(default)]
    pub uri: Option<String>,

    #[serde(default)]
    pub index: Option<String>,

    #[serde(default)]
    pub database: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WebhookConfig {
    pub url: String,

    #[serde(default = "default_webhook_events")]
    pub events: Vec<String>,

    #[serde(default)]
    pub secret: Option<String>,
}

fn default_webhook_events() -> Vec<String> {
    vec!["*".to_string()]
}

// ── Vault config ──────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct VaultConfig {
    pub provider: String,

    #[serde(default)]
    pub address: Option<String>,

    #[serde(default)]
    pub mount: Option<String>,
}

fn default_daemon_bind() -> String {
    "0.0.0.0:3000".to_string()
}

fn default_icm_prefix() -> String {
    "wshm".to_string()
}

// ── Global multi-repo config ──────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub daemon: GlobalDaemonConfig,

    #[serde(default)]
    pub ai: Option<AiConfig>,

    pub repos: Vec<RepoEntry>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct GlobalDaemonConfig {
    #[serde(default = "default_daemon_bind")]
    pub bind: String,

    #[serde(default)]
    pub webhook_secret: Option<String>,

    #[serde(default)]
    pub apply: bool,

    #[serde(default)]
    pub poll: bool,

    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,
}

fn default_poll_interval() -> u64 {
    30
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RepoEntry {
    /// Repository slug: "owner/repo"
    pub slug: String,

    /// Absolute path to the local checkout
    pub path: PathBuf,

    /// Per-repo apply override (inherits from [daemon].apply if not set)
    #[serde(default)]
    pub apply: Option<bool>,

    /// Per-repo webhook secret override
    #[serde(default)]
    pub secret: Option<String>,
}

impl GlobalConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read global config: {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse global config: {}", path.display()))?;
        if config.repos.is_empty() {
            anyhow::bail!("Global config has no [[repos]] entries");
        }
        Ok(config)
    }
}

impl Config {
    /// Load a Config for a specific repo from its working directory.
    /// Used by multi-repo daemon — slug is provided, no git detection needed.
    pub fn load_for_repo(repo_path: &Path, slug: &str) -> Result<Self> {
        let wshm_dir = repo_path.join(".wshm");
        let config_path = wshm_dir.join("config.toml");

        let mut config: Config = if config_path.exists() {
            let content = fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read {}", config_path.display()))?;
            toml::from_str(&content)
                .with_context(|| format!("Failed to parse {}", config_path.display()))?
        } else {
            Config::default()
        };

        config.wshm_dir = wshm_dir;

        let parts: Vec<&str> = slug.splitn(2, '/').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid repo slug: {slug} (expected owner/repo)");
        }
        config.repo_owner = parts[0].to_string();
        config.repo_name = parts[1].to_string();

        Ok(config)
    }

    pub fn fix_secret_env_vars(&self) -> Vec<String> {
        self.fix.secret_env.clone()
    }

    /// Resolve the AI model for a given pipeline.
    /// Pipeline-specific model overrides [ai].model.
    pub fn model_for(&self, pipeline: &str) -> &str {
        let override_model = match pipeline {
            "triage" => self.triage.model.as_deref(),
            "pr" => self.pr.model.as_deref(),
            "fix" => self.fix.model.as_deref(),
            _ => None,
        };
        override_model.unwrap_or(&self.ai.model)
    }

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
# Token from env var GITHUB_TOKEN or WSHM_TOKEN, or `gh auth token` (never stored in config)

[ai]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
# base_url = "https://custom-endpoint.example.com/v1/chat/completions"
#
# Supported providers:
#   anthropic  → ANTHROPIC_API_KEY
#   openai     → OPENAI_API_KEY
#   google     → GOOGLE_API_KEY or GEMINI_API_KEY
#   mistral    → MISTRAL_API_KEY
#   groq       → GROQ_API_KEY
#   deepseek   → DEEPSEEK_API_KEY
#   xai        → XAI_API_KEY
#   together   → TOGETHER_API_KEY
#   fireworks  → FIREWORKS_API_KEY
#   perplexity → PERPLEXITY_API_KEY
#   cohere     → COHERE_API_KEY or CO_API_KEY
#   openrouter → OPENROUTER_API_KEY
#   ollama     → no key needed (local)
#   azure      → AZURE_OPENAI_API_KEY + AZURE_OPENAI_ENDPOINT
#   custom     → WSHM_AI_API_KEY + base_url

[triage]
enabled = true
auto_fix = false
auto_fix_confidence = 0.85
# model = "claude-haiku-4-5-20251001"   # override: small model for triage
labels_bug = "bug"
labels_feature = "feature"
labels_duplicate = "duplicate"
labels_wontfix = "wontfix"
labels_needs_info = "needs-info"
# retriage_interval_hours = 24   # re-evaluate triaged issues every 24h (0 = disabled)

[pr]
enabled = true
auto_label = true
risk_labels = true
# model = "claude-sonnet-4-20250514"    # override: default model for PR analysis

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

[fix]
# model = "claude-sonnet-4-20250514"    # override: capable model for code generation
# trusted_authors_only = true   # Only auto-fix issues from repo collaborators
# allowed_users = ["alice", "bob"]  # Explicit whitelist (overrides trusted_authors_only)
# scan_diff = true               # Scan generated code for suspicious patterns
# draft_pr = true                # Create PRs as draft (require human review)
# base_branch = "main"           # Base branch for fix PRs (e.g. "develop")

# [update]
# enabled = false                        # Enable automatic update checks in daemon mode
# interval_hours = 6                     # Check interval
# auto_apply = false                     # Auto-install updates (daemon uses true)

# [branding]
# name = "wshm"                       # Bot display name in comments
# url = "https://github.com/pszymkowiak/wshm"  # Link in comment footers
# avatar_url = "https://example.com/logo.png"   # Optional avatar in headers
# tagline = "AI-powered repo assistant"          # Optional tagline
# command_prefix = "/wshm"             # Slash command prefix
# footer_template = "*{action} by [{name}]({url})*"  # Custom footer

# [vault]
# provider = "hashicorp"               # "hashicorp" | "aws" | "azure" | "gcp"
# address = "https://vault.example.com"
# mount = "secret"
# Auth from env: VAULT_TOKEN, VAULT_ROLE_ID, etc.

# [export.storage]
# provider = "s3"                      # "s3" | "azure" | "gcs"
# bucket = "wshm-logs"
# prefix = "repos/{repo}/"
# region = "eu-west-1"

# [export.database]
# provider = "elasticsearch"           # "elasticsearch" | "opensearch" | "postgresql" | "mongodb" | "mysql" | "mariadb"
# uri = "http://localhost:9200"        # or vault(secret/wshm/elastic-uri)
# index = "wshm-events"

# [[export.webhooks]]
# url = "https://hooks.example.com/wshm"
# events = ["FixApplied", "PrMerged"]
# secret = "your-hmac-secret"          # or vault(secret/wshm/webhook-hmac)

# [[export.webhooks]]
# url = "https://slack.example.com/webhook"
# events = ["*"]
"#;

        fs::write(&config_path, template)?;
        Ok(())
    }

    pub fn repo_slug(&self) -> String {
        format!("{}/{}", self.repo_owner, self.repo_name)
    }

    pub fn github_token(&self) -> Result<String> {
        let token = std::env::var("WSHM_TOKEN")
            .or_else(|_| std::env::var("GITHUB_TOKEN"))
            .or_else(|_| gh_auth_token())
            .context("No GitHub token found. Set GITHUB_TOKEN, WSHM_TOKEN, or authenticate with `gh auth login`")?;
        if token.trim().is_empty() {
            anyhow::bail!("GitHub token is empty. Check your GITHUB_TOKEN or WSHM_TOKEN environment variable.");
        }
        Ok(token)
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
            fix: FixConfig::default(),
            daemon: DaemonConfig::default(),
            branding: BrandingConfig::default(),
            update: UpdateConfig::default(),
            export: ExportConfig::default(),
            vault: None,
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
