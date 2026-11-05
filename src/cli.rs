use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "wshm", about = "Your repo's wish is my command.", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Skip GitHub sync, use cached data only
    #[arg(long, global = true)]
    pub offline: bool,

    /// Detailed output
    #[arg(long, short, global = true)]
    pub verbose: bool,

    /// JSON output for scripting
    #[arg(long, global = true)]
    pub json: bool,

    /// CSV output for spreadsheets
    #[arg(long, global = true)]
    pub csv: bool,

    /// Override detected repo (owner/repo)
    #[arg(long, global = true)]
    pub repo: Option<String>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Force full sync from GitHub
    Sync,

    /// Classify issues
    Triage(TriageArgs),

    /// Analyze pull requests
    Pr(PrArgs),

    /// Show ranked merge queue
    Queue(QueueArgs),

    /// Full cycle: sync + triage + analyze + queue + conflicts
    Run(RunArgs),

    /// PR health: detect duplicates, stale/zombie PRs
    Health(HealthArgs),

    /// Inline code review on PR diffs (AI-powered)
    Review(ReviewArgs),

    /// Auto-generate a PR fix from an issue (Claude Code / Codex / container)
    Fix(FixArgs),

    /// Detect and resolve conflicts
    Conflicts(ConflictArgs),

    /// Analyze codebase and propose improvement issues
    Improve(ImproveArgs),

    /// Generate a report (triage + PR analysis + queue)
    Report(ReportArgs),

    /// Generate changelog from merged PRs
    Changelog(ChangelogArgs),

    /// Generate metrics dashboard (HTML with charts)
    Dashboard(DashboardArgs),

    /// Show prioritized action plan (issues ranked by triage priority)
    Plan(PlanCliArgs),

    /// Send a priority summary notification
    Notify,

    /// Configuration management
    #[command(subcommand)]
    Config(ConfigCommand),

    /// Export repo context as LLM-ready markdown
    Context,

    /// Authenticate with GitHub and AI provider
    Login(LoginArgs),

    /// Check for updates and install latest release
    Update(UpdateArgs),

    /// Interactive TUI dashboard
    Tui,

    /// Migrate data from SQLite to PostgreSQL
    Migrate(MigrateArgs),

    /// Revert all wshm actions (remove comments, labels, clear results)
    Revert(RevertArgs),

    /// Backup database and config
    Backup(BackupArgs),

    /// Restore from a backup file
    Restore(RestoreArgs),

    /// Manage anonymous telemetry consent (GDPR)
    Telemetry(TelemetryArgs),

    /// Show daily digest summary (same data as Discord notifications)
    Summary,

    /// Start persistent daemon with webhook server and web UI
    Daemon(DaemonArgs),
}

#[derive(clap::Args)]
pub struct ConflictArgs {
    /// Attempt conflict resolution
    #[arg(long)]
    pub apply: bool,
}

#[derive(clap::Args)]
pub struct ReviewArgs {
    /// Review single PR by number
    #[arg(long)]
    pub pr: Option<u64>,

    /// Actually post review comments on GitHub (dry-run by default)
    #[arg(long)]
    pub apply: bool,
}

#[derive(clap::Args)]
pub struct ReportArgs {
    /// Output format: md, html, pdf
    #[arg(long, default_value = "md")]
    pub format: String,

    /// Output file path (defaults to wshm-report.<format>)
    #[arg(short, long)]
    pub output: Option<String>,
}

#[derive(clap::Args)]
pub struct FixArgs {
    /// Issue number to fix
    #[arg(long)]
    pub issue: u64,

    /// AI tool to use: claude-code, codex
    #[arg(long)]
    pub tool: Option<String>,

    /// Model override (e.g. claude-sonnet-4-20250514)
    #[arg(long)]
    pub model: Option<String>,

    /// Run inside a rootless Podman container (sandboxed)
    #[arg(long, alias = "docker")]
    pub docker: bool,

    /// Container image to use (default: wshm-sandbox:latest)
    #[arg(long)]
    pub image: Option<String>,

    /// Actually create the branch and PR (dry-run by default)
    #[arg(long)]
    pub apply: bool,
}

#[derive(clap::Args)]
pub struct ImproveArgs {
    /// Actually create issues on GitHub (dry-run by default)
    #[arg(long)]
    pub apply: bool,

    /// Maximum number of suggestions (default: 5)
    #[arg(long)]
    pub limit: Option<u32>,

    /// Automatically trigger auto-fix on created issues
    #[arg(long)]
    pub auto_fix: bool,
}

#[derive(clap::Args)]
pub struct ChangelogArgs {
    /// Number of days to look back (default: 30)
    #[arg(long, default_value = "30")]
    pub days: u64,

    /// Output format: md, json
    #[arg(long, default_value = "md")]
    pub format: String,
}

#[derive(clap::Args)]
pub struct DashboardArgs {
    /// Output file path (default: wshm-dashboard.html)
    #[arg(short, long)]
    pub output: Option<String>,
}

/// `plan` takes `--json` / `--csv` from the global flags.
#[derive(clap::Args)]
pub struct PlanCliArgs {
    /// Maximum number of items per repo (default: 50)
    #[arg(long, default_value = "50")]
    pub limit: usize,
}

#[derive(clap::Args)]
pub struct TelemetryArgs {
    /// Accept anonymous telemetry
    #[arg(long, conflicts_with_all = ["decline", "status"])]
    pub accept: bool,

    /// Decline anonymous telemetry
    #[arg(long, conflicts_with_all = ["accept", "status"])]
    pub decline: bool,

    /// Show current telemetry consent state (default)
    #[arg(long)]
    pub status: bool,
}

#[derive(clap::Args)]
pub struct RevertArgs {
    /// Actually remove comments and labels (dry-run by default)
    #[arg(long)]
    pub apply: bool,
}

#[derive(clap::Args)]
pub struct BackupArgs {
    /// Output file path (default: .wshm/backup-YYYY-MM-DD.tar.gz)
    #[arg(short, long)]
    pub output: Option<String>,

    /// Include logs directory
    #[arg(long, default_value = "false")]
    pub include_logs: bool,
}

#[derive(clap::Args)]
pub struct RestoreArgs {
    /// Backup file to restore from
    pub file: String,

    /// Force overwrite without confirmation
    #[arg(short, long)]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct MigrateArgs {
    /// Target database provider
    #[arg(long, default_value = "postgresql")]
    pub to: String,

    /// Target database URI
    #[arg(long)]
    pub uri: String,

    /// Migrate all repos from global config (otherwise just current repo)
    #[arg(long)]
    pub all: bool,

    /// Path to global config (for --all mode)
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,
}

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// Actually install the update (check only by default)
    #[arg(long)]
    pub apply: bool,
}

#[derive(clap::Args)]
pub struct TriageArgs {
    /// Triage single issue by number
    #[arg(long)]
    pub issue: Option<u64>,

    /// Actually perform actions (dry-run by default)
    #[arg(long)]
    pub apply: bool,

    /// Re-evaluate previously triaged issues (stale results)
    #[arg(long)]
    pub retriage: bool,
}

#[derive(clap::Args)]
pub struct PrArgs {
    /// Analyze single PR by number
    #[arg(long)]
    pub pr: Option<u64>,

    /// Actually perform actions (dry-run by default)
    #[arg(long)]
    pub apply: bool,
}

#[derive(clap::Args)]
pub struct QueueArgs {
    /// Merge top PR if above threshold
    #[arg(long)]
    pub apply: bool,
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// Actually perform actions (dry-run by default)
    #[arg(long)]
    pub apply: bool,
}

#[derive(clap::Args)]
pub struct HealthArgs {
    /// Minimum days without activity to flag as stale
    #[arg(long, default_value = "14")]
    pub stale_days: i64,
}

#[derive(clap::Args)]
pub struct LoginArgs {
    /// Only setup GitHub authentication
    #[arg(long)]
    pub github: bool,

    /// Only setup AI provider authentication (API key)
    #[arg(long)]
    pub ai: bool,

    /// Login with Claude Max/Pro/Team (OAuth, uses your subscription)
    #[arg(long)]
    pub claude: bool,

    /// Only setup license key
    #[arg(long)]
    pub license: bool,

    /// Show current authentication status
    #[arg(long)]
    pub status: bool,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Create .wshm/config.toml template
    Init,
}

#[derive(clap::Args, Clone)]
pub struct DaemonArgs {
    /// Path to global multi-repo config (e.g. ~/.wshm/global.toml)
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Bind address (default: 0.0.0.0:3000)
    #[arg(long)]
    pub bind: Option<String>,

    /// Actually perform actions (dry-run by default)
    #[arg(long)]
    pub apply: bool,

    /// Webhook secret (overrides config/env)
    #[arg(long, env = "WSHM_WEBHOOK_SECRET")]
    pub secret: Option<String>,

    /// Use polling instead of webhooks (no public IP needed)
    #[arg(long)]
    pub poll: bool,

    /// Polling interval in seconds (default: 30)
    #[arg(long, default_value = "30")]
    pub poll_interval: u64,

    /// Disable the HTTP webhook server (use with --poll)
    #[arg(long)]
    pub no_server: bool,

    /// Install wshm daemon as a systemd service
    #[arg(long)]
    pub install: bool,

    /// Uninstall the wshm systemd service
    #[arg(long)]
    pub uninstall: bool,

    /// Override working directory for systemd (default: current dir)
    #[arg(long)]
    pub workdir: Option<String>,

    /// Override repo for systemd (default: from config)
    #[arg(long)]
    pub repo: Option<String>,
}
