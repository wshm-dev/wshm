use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::Cli;

/// Resolved GitHub App credentials — never part of `config.toml` (secrets
/// only live in the encrypted store / env, per [`Config::github_app_auth_optional`]).
#[derive(Debug, Clone)]
pub struct GithubAppAuth {
    pub app_id: u64,
    pub installation_id: u64,
    pub private_key_pem: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub github: GitHubConfig,

    /// Git provider: "github" (default), "gitlab", "gitea", "forgejo",
    /// "azure-devops". Non-GitHub tokens come from the secret store
    /// (`gitlab_token`, `gitea_token`, `forgejo_token`, `azure_devops_token`)
    /// or env (`GITLAB_TOKEN`, `GITEA_TOKEN`, `FORGEJO_TOKEN`,
    /// `AZURE_DEVOPS_TOKEN`), never from this file. Azure DevOps repo slug:
    /// `org/project/repo`.
    #[serde(default)]
    pub git_provider: Option<String>,

    /// Git platform URL for self-hosted (e.g. "https://gitlab.company.com")
    #[serde(default)]
    pub git_url: Option<String>,

    /// Azure DevOps settings (only read when `git_provider = "azure-devops"`).
    #[serde(default)]
    pub azure_devops: AzureDevOpsConfig,

    #[serde(default)]
    pub ai: AiConfig,

    #[serde(default)]
    pub triage: TriageConfig,

    #[serde(default)]
    pub pr: PrConfig,

    #[serde(default)]
    pub queue: QueueConfig,

    #[serde(default)]
    pub scoring: ScoringConfig,

    #[serde(default)]
    pub conflicts: ConflictConfig,

    #[serde(default)]
    pub sync: SyncConfig,

    #[serde(default)]
    pub fix: FixConfig,

    #[serde(default)]
    pub assign: AssignConfig,

    #[serde(default)]
    pub daemon: DaemonConfig,

    #[serde(default)]
    pub branding: BrandingConfig,

    #[serde(default)]
    pub update: UpdateConfig,

    #[serde(default)]
    pub export: ExportConfig,

    #[serde(default)]
    pub notify: NotifyConfig,

    #[serde(default)]
    pub vault: Option<VaultConfig>,

    #[serde(default)]
    pub license: LicenseConfig,

    #[serde(default)]
    pub web: WebConfig,

    /// Database backend: "sqlite" (default) or "postgresql".
    #[serde(default)]
    pub database: Option<DatabaseConfig>,

    /// Labels that wshm must never apply (blacklist).
    #[serde(default)]
    pub labels_blacklist: Vec<String>,

    /// Issue numbers to never triage or touch.
    #[serde(default)]
    pub issues_blacklist: Vec<u64>,

    /// PR numbers to never analyze or touch.
    #[serde(default)]
    pub prs_blacklist: Vec<u64>,

    /// Label definitions with conditions. Fed to AI for better label selection.
    #[serde(default)]
    pub labels: Vec<LabelDef>,

    /// `[[skills]]` — versionable SEED for the DB-backed skills list. Copied
    /// into `app_settings` at startup by [`seed_skills`]; after that the DB
    /// (edited in Settings → Skills) is the source of truth and these
    /// entries never overwrite it. Pipelines read [`load_skills`], not this.
    #[serde(default)]
    pub skills: Vec<SkillDef>,

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

/// `[azure_devops]` — Azure DevOps specifics.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AzureDevOpsConfig {
    /// Work item types synced and triaged as issues (default: Issue, Bug).
    /// Overridable at runtime, per repo and shared across pods, by the
    /// `azure_work_item_types` app setting (JSON array of strings).
    #[serde(default = "default_azure_work_item_types")]
    pub work_item_types: Vec<String>,
}

fn default_azure_work_item_types() -> Vec<String> {
    vec!["Issue".to_string(), "Bug".to_string()]
}

impl Default for AzureDevOpsConfig {
    fn default() -> Self {
        Self {
            work_item_types: default_azure_work_item_types(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AiConfig {
    #[serde(default = "default_ai_provider")]
    pub provider: String,

    #[serde(default = "default_ai_model")]
    pub model: String,

    /// Optional base URL override (for custom endpoints, proxies, etc.)
    #[serde(default)]
    pub base_url: Option<String>,

    /// "Related history" retrieval (`[ai.rag]`): closed issues and merged
    /// PRs found by full-text search are appended to the triage and
    /// PR-analysis prompts. Lexical only (FTS5 / tsvector), no embeddings.
    #[serde(default)]
    pub rag: RagConfig,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            provider: default_ai_provider(),
            model: default_ai_model(),
            base_url: None,
            rag: RagConfig::default(),
        }
    }
}

/// `[ai.rag]` — see `ai::related`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RagConfig {
    /// Master switch (default: true). Set to false to get the pre-#108
    /// prompts back.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Related items kept in the prompt (default: 5).
    #[serde(default = "default_rag_top_k")]
    pub top_k: usize,
    /// Search terms extracted from title + body (default: 8).
    #[serde(default = "default_rag_max_terms")]
    pub max_terms: usize,
    /// Size cap of the rendered block, in characters (default: 3000).
    #[serde(default = "default_rag_max_chars")]
    pub max_chars: usize,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            top_k: default_rag_top_k(),
            max_terms: default_rag_max_terms(),
            max_chars: default_rag_max_chars(),
        }
    }
}

impl RagConfig {
    /// Environment overrides, applied after the TOML is parsed (env beats
    /// file, per the documented precedence). Stateless deployments have no
    /// per-repo `config.toml` to edit, so these are the practical knobs:
    ///
    /// - `WSHM_RAG_ENABLED`   true/false (also 1/0, yes/no, on/off)
    /// - `WSHM_RAG_TOP_K`     items kept in the prompt
    /// - `WSHM_RAG_MAX_TERMS` search terms extracted
    /// - `WSHM_RAG_MAX_CHARS` size cap of the block
    ///
    /// Unset or unparsable values leave the TOML/default value untouched
    /// (a warning is logged for unparsable ones).
    pub fn apply_env_overrides(&mut self) {
        if let Some(v) = env_non_empty("WSHM_RAG_ENABLED") {
            match v.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => self.enabled = true,
                "0" | "false" | "no" | "off" => self.enabled = false,
                other => tracing::warn!("WSHM_RAG_ENABLED={other:?} is not a boolean; ignored"),
            }
        }
        for (name, slot) in [
            ("WSHM_RAG_TOP_K", &mut self.top_k),
            ("WSHM_RAG_MAX_TERMS", &mut self.max_terms),
            ("WSHM_RAG_MAX_CHARS", &mut self.max_chars),
        ] {
            if let Some(v) = env_non_empty(name) {
                match v.parse::<usize>() {
                    Ok(n) => *slot = n,
                    Err(_) => tracing::warn!("{name}={v:?} is not an integer; ignored"),
                }
            }
        }
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn default_rag_top_k() -> usize {
    5
}
fn default_rag_max_terms() -> usize {
    8
}
fn default_rag_max_chars() -> usize {
    3000
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

    /// Minimum confidence to apply labels and post comments (default: 0.5)
    #[serde(default = "default_triage_confidence")]
    pub triage_confidence: f64,

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

    /// Append AI-generated "Suggested Actions" to triage comments (default: true).
    /// Set to false to omit the section from comments and CLI output, and to avoid
    /// asking the AI to generate them (saves tokens).
    #[serde(default = "default_true")]
    pub suggested_actions: bool,

    /// Override the AI system prompt for triage. If not set, uses the built-in default.
    #[serde(default)]
    pub system_prompt: Option<String>,
}

impl Default for TriageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_fix: false,
            auto_fix_confidence: default_confidence(),
            triage_confidence: default_triage_confidence(),
            model: None,
            labels_bug: default_label_bug(),
            labels_feature: default_label_feature(),
            labels_duplicate: default_label_duplicate(),
            labels_wontfix: default_label_wontfix(),
            labels_needs_info: default_label_needs_info(),
            retriage_interval_hours: 0,
            suggested_actions: true,
            system_prompt: None,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_confidence() -> f64 {
    0.85
}
fn default_triage_confidence() -> f64 {
    0.5
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

    /// Override the AI system prompt for PR analysis.
    #[serde(default)]
    pub system_prompt: Option<String>,

    /// `[pr.review_diagrams]`: architecture diagram added to the PR analysis
    /// comment, plus an optional interactive page. Can be overridden per repo
    /// in the DB, see [`load_review_diagrams`].
    #[serde(default)]
    pub review_diagrams: ReviewDiagramsConfig,
}

impl Default for PrConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_label: true,
            risk_labels: true,
            model: None,
            system_prompt: None,
            review_diagrams: ReviewDiagramsConfig::default(),
        }
    }
}

/// How much prose the diagram carries (story, captions, node details).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramText {
    #[default]
    Short,
    Normal,
    Full,
}

/// What the PR comment shows of the diagram.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramComment {
    /// Mermaid flowchart + story + walkthrough in the comment.
    #[default]
    Mermaid,
    /// Nothing but a link to the interactive page (when one is published).
    None,
}

/// Where the interactive HTML page is published.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagramPublishTarget {
    #[default]
    Off,
    Local,
    GithubPages,
    Wshm,
    S3,
    Gcs,
    Azure,
}

/// `[pr.review_diagrams.publish]`. No secret here: GitHub Pages uses the
/// repo's GitHub credentials, object stores their own credential chain.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct DiagramPublishConfig {
    pub target: DiagramPublishTarget,
    /// Key prefix (directory) of the published pages.
    pub prefix: String,
    /// Base URL the pages are served from; the link is `public_url/key`.
    /// For `wshm` it is the daemon's external URL. Without it (local) the
    /// page is written but no link is added to the comment.
    pub public_url: Option<String>,
    /// `local`: directory the pages are written to.
    pub dir: Option<String>,
    /// `github-pages`: `owner/repo` whose `gh-pages` branch receives the pages.
    pub pages_repo: Option<String>,
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub container: Option<String>,
    pub account: Option<String>,
    /// Delete pages not updated for this many days (0 = keep until the PR
    /// closes). Pages are always deleted when the PR is closed or merged.
    pub retention_days: u32,
    /// Max published pages per repo; beyond it the least recently updated
    /// pages are evicted first.
    pub max_pages: u32,
    /// Max diff size embedded in the page (KB). Above it the viewer links to
    /// the PR's "Files changed" page instead.
    pub max_diff_kb: u32,
    /// Allow a public target (github-pages, object stores, local with a
    /// public_url) for a private repo. Off by default: the page embeds the diff.
    pub allow_private: bool,
}

impl Default for DiagramPublishConfig {
    fn default() -> Self {
        Self {
            target: DiagramPublishTarget::Off,
            prefix: "wshm-diagrams".to_string(),
            public_url: None,
            dir: None,
            pages_repo: None,
            bucket: None,
            region: None,
            endpoint: None,
            container: None,
            account: None,
            retention_days: 30,
            max_pages: 200,
            max_diff_kb: 1024,
            allow_private: false,
        }
    }
}

/// `[pr.review_diagrams]`. Resolution: DB (per repo) > TOML > these defaults.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ReviewDiagramsConfig {
    pub enabled: bool,
    /// Diagram kinds. Only "architecture" is implemented; "flow", "changes",
    /// "delta" and "dependency" are accepted and ignored with a warning.
    pub kinds: Vec<String>,
    pub text: DiagramText,
    /// Language of every human-readable text (ISO code, e.g. "en", "fr").
    pub language: String,
    pub comment: DiagramComment,
    /// Embed the PR diff in the interactive page (per-component diff drawer).
    pub diff_viewer: bool,
    /// Skip generation for PRs without activity for this many days (0 = no
    /// limit). A new push re-enables them.
    pub active_days: u32,
    /// Max size of the Mermaid block in the comment (chars). Forgejo/Gitea
    /// refuse to render more than 5000 by default; above it edge labels,
    /// then unchanged nodes are dropped, then a link replaces the diagram.
    pub max_mermaid_chars: u32,
    /// Time budget of the whole diagram step (AI calls + publication), in
    /// seconds. It runs after the analysis comment is posted, in the
    /// background; past the budget it is abandoned (the AI CLI is killed).
    pub timeout_secs: u32,
    /// Max per-file summary AI calls for a large diff (> 40 KB).
    pub max_summary_calls: u32,
    pub publish: DiagramPublishConfig,
}

impl Default for ReviewDiagramsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            kinds: vec!["architecture".to_string()],
            text: DiagramText::Short,
            language: "en".to_string(),
            comment: DiagramComment::Mermaid,
            diff_viewer: true,
            active_days: 14,
            max_mermaid_chars: 5000,
            timeout_secs: 120,
            max_summary_calls: 3,
            publish: DiagramPublishConfig::default(),
        }
    }
}

/// Diagram kinds the config accepts. Only the first one is implemented.
pub const DIAGRAM_KINDS: &[&str] = &["architecture", "flow", "changes", "delta", "dependency"];

/// Recursively overlay `over` onto `base` (objects merged key by key, any
/// other value replaced).
fn merge_json(base: &mut serde_json::Value, over: &serde_json::Value) {
    match (base, over) {
        (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                merge_json(b.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// Keys of `over` that `reference` does not know (dotted paths). Used to
/// reject typos in a per-repo override instead of silently ignoring them.
pub fn unknown_json_keys(reference: &serde_json::Value, over: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let (Some(r), Some(o)) = (reference.as_object(), over.as_object()) {
        for (k, v) in o {
            match r.get(k) {
                None => out.push(k.clone()),
                Some(rv) if rv.is_object() => out.extend(
                    unknown_json_keys(rv, v)
                        .into_iter()
                        .map(|s| format!("{k}.{s}")),
                ),
                Some(_) => {}
            }
        }
    }
    out
}

/// `[pr.review_diagrams]` keys that only the TOML may set: where files are
/// written or committed, and the private-repo safeguard. A per-repo DB
/// override (editable from the dashboard) must not redirect output or
/// expose private code.
pub const REVIEW_DIAGRAMS_TOML_ONLY: &[&str] =
    &["publish.dir", "publish.pages_repo", "publish.allow_private"];

/// TOML-only keys present in a per-repo override.
pub fn toml_only_keys(over: &serde_json::Value) -> Vec<String> {
    REVIEW_DIAGRAMS_TOML_ONLY
        .iter()
        .filter(|k| {
            let mut v = over;
            for part in k.split('.') {
                match v.get(part) {
                    Some(x) => v = x,
                    None => return false,
                }
            }
            true
        })
        .map(|k| k.to_string())
        .collect()
}

/// Apply a per-repo override (JSON object, possibly partial) on top of the
/// TOML value. TOML-only keys in the override are ignored. Errors when the
/// merged value is not a valid config.
pub fn apply_review_diagrams_override(
    toml: &ReviewDiagramsConfig,
    over: &serde_json::Value,
) -> Result<ReviewDiagramsConfig> {
    let mut over = over.clone();
    for k in REVIEW_DIAGRAMS_TOML_ONLY {
        if let Some((section, key)) = k.split_once('.') {
            if let Some(obj) = over.get_mut(section).and_then(|v| v.as_object_mut()) {
                if obj.remove(key).is_some() {
                    tracing::warn!("review_diagrams DB override: '{k}' is TOML-only, ignored");
                }
            }
        }
    }
    let mut base = serde_json::to_value(toml)?;
    merge_json(&mut base, &over);
    Ok(serde_json::from_value(base)?)
}

/// The repo's effective `[pr.review_diagrams]`: the DB override (Settings →
/// Diagrams, shared by every pod since it is read on each pass) merged over
/// the TOML value. A corrupt override is logged and ignored.
pub fn load_review_diagrams(
    config: &Config,
    db: &dyn crate::db::backend::DatabaseBackend,
) -> ReviewDiagramsConfig {
    let toml = &config.pr.review_diagrams;
    let raw = db
        .get_app_setting(crate::db::settings::REVIEW_DIAGRAMS_KEY)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty() && s.trim() != "null");
    let Some(raw) = raw else {
        return toml.clone();
    };
    match serde_json::from_str::<serde_json::Value>(&raw)
        .map_err(anyhow::Error::from)
        .and_then(|v| apply_review_diagrams_override(toml, &v))
    {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Ignoring invalid review_diagrams override in the DB: {e:#}");
            toml.clone()
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

// ---------------------------------------------------------------------------
// Scoring (PR queue + issue ranking)
// ---------------------------------------------------------------------------

/// Tunable weights for the PR queue and issue ranking algorithms.
///
/// Defaults are chosen so the typical PR scores 0..30 (review-ready),
/// blocked PRs score below 0, and the merge queue threshold of 15
/// (`[queue].merge_threshold`) lands on PRs with green CI + an
/// approval-equivalent label or a fixed bug.
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct ScoringConfig {
    #[serde(default)]
    pub pr: PrScoringConfig,

    #[serde(default)]
    pub issue: IssueScoringConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PrScoringConfig {
    /// Reward for green CI.
    #[serde(default = "pr_default_ci_green")]
    pub ci_green: i32,
    /// Penalty for failing CI (was missing pre-2026-05).
    #[serde(default = "pr_default_ci_failure")]
    pub ci_failure: i32,
    /// Penalty for pending CI (slows merge until checks land).
    #[serde(default = "pr_default_ci_pending")]
    pub ci_pending: i32,

    /// Reward for a clean mergeable state.
    #[serde(default = "pr_default_mergeable")]
    pub mergeable: i32,
    /// Penalty for a known conflict against the base branch.
    #[serde(default = "pr_default_conflict")]
    pub conflict: i32,

    /// Reward when the PR body links an issue ("fixes #N", "closes #N", ...).
    #[serde(default = "pr_default_linked_issue")]
    pub linked_issue: i32,

    /// Boosts driven by labels on the PR.
    #[serde(default = "pr_default_label_critical")]
    pub label_priority_critical: i32,
    #[serde(default = "pr_default_label_high")]
    pub label_priority_high: i32,
    #[serde(default = "pr_default_label_medium")]
    pub label_priority_medium: i32,
    #[serde(default = "pr_default_label_bug")]
    pub label_bug: i32,
    /// Block-style labels that should sink a PR even if CI is green.
    #[serde(default = "pr_default_label_blocked")]
    pub label_blocked: i32,

    /// Per-day age bonus, capped by `age_max_days` so old PRs don't dominate.
    #[serde(default = "pr_default_age_bonus_max")]
    pub age_bonus_max: i32,
    #[serde(default = "pr_default_age_decay_after_days")]
    pub age_decay_after_days: i64,
    /// Penalty applied once a PR is older than the decay window.
    #[serde(default = "pr_default_age_decay_penalty")]
    pub age_decay_penalty: i32,

    /// Reward for recent activity (commit / comment within 24h).
    #[serde(default = "pr_default_momentum_recent")]
    pub momentum_recent: i32,
    /// Penalty for total dormancy (no update >30 days).
    #[serde(default = "pr_default_momentum_stale")]
    pub momentum_stale: i32,
    /// Threshold (days) under which a PR counts as "recent".
    #[serde(default = "pr_default_recent_days")]
    pub recent_days: i64,
    /// Threshold (days) above which a PR counts as "stale".
    #[serde(default = "pr_default_stale_days")]
    pub stale_days: i64,
}

impl Default for PrScoringConfig {
    fn default() -> Self {
        Self {
            ci_green: pr_default_ci_green(),
            ci_failure: pr_default_ci_failure(),
            ci_pending: pr_default_ci_pending(),
            mergeable: pr_default_mergeable(),
            conflict: pr_default_conflict(),
            linked_issue: pr_default_linked_issue(),
            label_priority_critical: pr_default_label_critical(),
            label_priority_high: pr_default_label_high(),
            label_priority_medium: pr_default_label_medium(),
            label_bug: pr_default_label_bug(),
            label_blocked: pr_default_label_blocked(),
            age_bonus_max: pr_default_age_bonus_max(),
            age_decay_after_days: pr_default_age_decay_after_days(),
            age_decay_penalty: pr_default_age_decay_penalty(),
            momentum_recent: pr_default_momentum_recent(),
            momentum_stale: pr_default_momentum_stale(),
            recent_days: pr_default_recent_days(),
            stale_days: pr_default_stale_days(),
        }
    }
}

fn pr_default_ci_green() -> i32 {
    25
}
fn pr_default_ci_failure() -> i32 {
    -30
}
fn pr_default_ci_pending() -> i32 {
    -5
}
fn pr_default_mergeable() -> i32 {
    2
}
fn pr_default_conflict() -> i32 {
    // Must outweigh every other positive signal a PR can rack up (ci_green
    // 25 + age_bonus_max 10 = 35) so an old, green-CI PR with a real GitHub
    // merge conflict can never coast above a clean PR on age alone — a
    // conflict means it needs a rebase before it's "ready", full stop.
    -35
}
fn pr_default_linked_issue() -> i32 {
    5
}
fn pr_default_label_critical() -> i32 {
    30
}
fn pr_default_label_high() -> i32 {
    15
}
fn pr_default_label_medium() -> i32 {
    5
}
fn pr_default_label_bug() -> i32 {
    5
}
fn pr_default_label_blocked() -> i32 {
    -50
}
fn pr_default_age_bonus_max() -> i32 {
    10
}
fn pr_default_age_decay_after_days() -> i64 {
    60
}
fn pr_default_age_decay_penalty() -> i32 {
    -15
}
fn pr_default_momentum_recent() -> i32 {
    5
}
fn pr_default_momentum_stale() -> i32 {
    -10
}
fn pr_default_recent_days() -> i64 {
    1
}
fn pr_default_stale_days() -> i64 {
    30
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IssueScoringConfig {
    /// Boost from the AI-assigned priority field.
    #[serde(default = "issue_default_ai_critical")]
    pub ai_priority_critical: i32,
    #[serde(default = "issue_default_ai_high")]
    pub ai_priority_high: i32,
    #[serde(default = "issue_default_ai_medium")]
    pub ai_priority_medium: i32,
    /// AI confidence threshold below which the AI-priority boost is halved.
    #[serde(default = "issue_default_low_conf_threshold")]
    pub low_confidence_threshold: f64,

    /// Boost per `+1` reaction, capped by `reaction_max_bonus`.
    #[serde(default = "issue_default_reaction_per_plus1")]
    pub reaction_per_plus1: i32,
    #[serde(default = "issue_default_reaction_max")]
    pub reaction_max_bonus: i32,

    /// Label-driven boosts/penalties on issues.
    #[serde(default = "issue_default_label_security")]
    pub label_security: i32,
    #[serde(default = "issue_default_label_regression")]
    pub label_regression: i32,
    #[serde(default = "issue_default_label_bug")]
    pub label_bug: i32,
    #[serde(default = "issue_default_label_first")]
    pub label_first_issue: i32,
    #[serde(default = "issue_default_label_blocked")]
    pub label_blocked: i32,

    /// Recency window — issues updated within `recent_days` get the bonus.
    #[serde(default = "issue_default_recent_days")]
    pub recent_days: i64,
    #[serde(default = "issue_default_recent_bonus")]
    pub recent_bonus: i32,
    /// Issues without activity for `stale_days` get the penalty.
    #[serde(default = "issue_default_stale_days")]
    pub stale_days: i64,
    #[serde(default = "issue_default_stale_penalty")]
    pub stale_penalty: i32,
}

impl Default for IssueScoringConfig {
    fn default() -> Self {
        Self {
            ai_priority_critical: issue_default_ai_critical(),
            ai_priority_high: issue_default_ai_high(),
            ai_priority_medium: issue_default_ai_medium(),
            low_confidence_threshold: issue_default_low_conf_threshold(),
            reaction_per_plus1: issue_default_reaction_per_plus1(),
            reaction_max_bonus: issue_default_reaction_max(),
            label_security: issue_default_label_security(),
            label_regression: issue_default_label_regression(),
            label_bug: issue_default_label_bug(),
            label_first_issue: issue_default_label_first(),
            label_blocked: issue_default_label_blocked(),
            recent_days: issue_default_recent_days(),
            recent_bonus: issue_default_recent_bonus(),
            stale_days: issue_default_stale_days(),
            stale_penalty: issue_default_stale_penalty(),
        }
    }
}

fn issue_default_ai_critical() -> i32 {
    50
}
fn issue_default_ai_high() -> i32 {
    25
}
fn issue_default_ai_medium() -> i32 {
    10
}
fn issue_default_low_conf_threshold() -> f64 {
    0.5
}
fn issue_default_reaction_per_plus1() -> i32 {
    1
}
fn issue_default_reaction_max() -> i32 {
    20
}
fn issue_default_label_security() -> i32 {
    30
}
fn issue_default_label_regression() -> i32 {
    15
}
fn issue_default_label_bug() -> i32 {
    8
}
fn issue_default_label_first() -> i32 {
    3
}
fn issue_default_label_blocked() -> i32 {
    -10
}
fn issue_default_recent_days() -> i64 {
    7
}
fn issue_default_recent_bonus() -> i32 {
    5
}
fn issue_default_stale_days() -> i64 {
    90
}
fn issue_default_stale_penalty() -> i32 {
    -10
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

// ── Label definitions ─────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LabelDef {
    /// Label name (e.g. "bug", "priority:high")
    pub name: String,

    /// What this label means
    #[serde(default)]
    pub description: Option<String>,

    /// When to apply this label
    #[serde(default)]
    pub when: Option<String>,

    /// Label color (hex, e.g. "d73a4a")
    #[serde(default)]
    pub color: Option<String>,
}

impl Config {
    /// Build a prompt fragment describing the available labels for the AI.
    pub fn labels_prompt(&self) -> String {
        if self.labels.is_empty() {
            return String::new();
        }
        let mut out = String::from("\n## Available labels (use ONLY these):\n");
        for label in &self.labels {
            out.push_str(&format!("- **{}**", label.name));
            if let Some(ref desc) = label.description {
                out.push_str(&format!(": {desc}"));
            }
            if let Some(ref when) = label.when {
                out.push_str(&format!(" — Apply when: {when}"));
            }
            out.push('\n');
        }
        out.push_str("\nDo NOT invent labels outside this list.\n");
        out
    }
}

/// Build the AI-prompt fragment listing the configured "grand domains".
///
/// Source is the DB (K/V setting) — NOT the TOML config — because wshm-pro runs
/// as stateless pods (see the `app_settings` table). The pipeline fetches the
/// domain list + optional custom prompt from the DB and passes them here.
/// Returns empty when no domains are configured (the review omits the step).
pub fn domains_prompt(domains: &[DomainDef], custom: Option<&str>) -> String {
    if domains.is_empty() {
        return String::new();
    }
    if let Some(custom) = custom {
        if !custom.trim().is_empty() {
            return format!("\n{custom}\n");
        }
    }
    let mut out = String::from(
        "\n## Grand domains — broad areas of the product/codebase. Tag the PR/issue with EVERY domain it touches.\nKnown domains (reuse these when they fit):\n",
    );
    for d in domains {
        out.push_str(&format!("- **{}**", d.name));
        if let Some(ref desc) = d.description {
            out.push_str(&format!(": {desc}"));
        }
        out.push('\n');
    }
    out.push_str("\nReturn them in the `domains` array (short lowercase slugs, e.g. \"codex\", \"bun\"). Reuse a known domain whenever it fits; only introduce a NEW domain name if none of the above applies.\n");
    out
}

/// A named block of standing instructions the AI review should follow when
/// it applies — the same idea as an Anthropic Agent Skill (a reusable
/// capability the model loads when relevant), scoped here to wshm's own
/// triage/PR-review calls rather than a whole Claude session. Users write
/// these in Settings; `skills_prompt` renders the ones that match the
/// current pipeline into the user prompt, right after domains.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SkillDef {
    /// Short human name (e.g. "rust-error-handling").
    pub name: String,

    /// One-line summary shown in the Settings list — NOT sent to the AI.
    #[serde(default)]
    pub description: Option<String>,

    /// The actual instructions appended to the prompt verbatim.
    pub content: String,

    /// Which pipelines this skill applies to: any of [`SKILL_PIPELINES`]
    /// ("triage", "pr_review", "review" = inline code review, "diagrams").
    /// Empty means all of them.
    #[serde(default)]
    pub pipelines: Vec<String>,

    /// Skills can be authored but temporarily switched off without deleting them.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Pipeline names a [`SkillDef::pipelines`] entry may target.
pub const SKILL_PIPELINES: &[&str] = &["triage", "pr_review", "review", "diagrams"];

/// Strip pipeline values that aren't in [`SKILL_PIPELINES`], returning one
/// human-readable warning per change. A skill whose every pipeline was
/// unknown is disabled rather than left with an empty list (empty = ALL
/// pipelines, which would widen a typo into "apply everywhere").
pub fn sanitize_skill_pipelines(skill: &mut SkillDef) -> Vec<String> {
    let mut warnings = Vec::new();
    let had_any = !skill.pipelines.is_empty();
    let name = skill.name.clone();
    skill.pipelines.retain(|p| {
        let ok = SKILL_PIPELINES.contains(&p.as_str());
        if !ok {
            warnings.push(format!("skill '{name}': unknown pipeline '{p}' removed"));
        }
        ok
    });
    if had_any && skill.pipelines.is_empty() && skill.enabled {
        skill.enabled = false;
        warnings.push(format!(
            "skill '{name}': no known pipeline left, disabled (expected any of {SKILL_PIPELINES:?})"
        ));
    }
    warnings
}

/// Build the AI-prompt fragment listing the skills that apply to `pipeline`
/// (one of [`SKILL_PIPELINES`]). Mirrors [`domains_prompt`]'s DB-backed,
/// stateless-pods-safe pattern — see its doc comment for why this isn't TOML.
/// Returns empty when no enabled skill matches.
pub fn skills_prompt(skills: &[SkillDef], pipeline: &str) -> String {
    let matching: Vec<&SkillDef> = skills
        .iter()
        .filter(|s| {
            s.enabled && (s.pipelines.is_empty() || s.pipelines.iter().any(|p| p == pipeline))
        })
        .collect();
    if matching.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n## Skills — standing instructions to apply to this review:\n");
    for skill in matching {
        out.push_str(&format!("\n### {}\n{}\n", skill.name, skill.content.trim()));
    }
    out
}

/// Load this repo's configured Skills, falling back to [`default_skills`]
/// when the repo has never saved its own list (`SKILLS_KEY` unset). Once a
/// repo saves ANY list via Settings — including an empty one — that becomes
/// its persisted choice and the default is no longer consulted for it.
pub fn load_skills(db: &dyn crate::db::backend::DatabaseBackend) -> Vec<SkillDef> {
    match db
        .get_app_setting(crate::db::settings::SKILLS_KEY)
        .ok()
        .flatten()
    {
        Some(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        None => default_skills(),
    }
}

/// The single Skill pre-seeded for a repo that has never configured its own
/// skills list (i.e. the `SKILLS_KEY` app_setting has never been written).
/// Applies to every pipeline by default; repos disable or edit it like any
/// other skill via Settings — once a repo saves ANY skills list (even an
/// empty one), that repo's own choice is persisted and this default is no
/// longer consulted for it.
pub fn default_skills() -> Vec<SkillDef> {
    vec![SkillDef {
        name: "core-review-checklist".to_string(),
        description: Some(
            "Baseline checks applied to every AI review pass by default.".to_string(),
        ),
        content: "Flag concrete defects only: real bugs, security issues (injection, auth \
bypass, leaked secrets, unsafe deserialization), correctness errors, and obvious data \
races/resource leaks. Do not nitpick style, formatting, or naming that a linter already \
enforces. Do not suggest speculative refactors or hypothetical future-proofing. If a change \
looks correct and reasonably simple, say so briefly instead of inventing issues."
            .to_string(),
        pipelines: vec![],
        enabled: true,
    }]
}

/// What [`seed_skills`] did, for logging and tests.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillSeedOutcome {
    /// Names written into the DB list by this call.
    pub added: Vec<String>,
    /// True when this call created the `SKILLS_KEY` row (first ever seed).
    pub created: bool,
}

/// Seed the DB-backed skills list from the TOML `[[skills]]` (see
/// [`Config::skills`]). TOML is the versionable default, the DB the source of
/// truth once seeded:
///
/// - `SKILLS_KEY` never written → write the TOML skills, or [`default_skills`]
///   when the TOML defines none.
/// - `SKILLS_KEY` exists → append only TOML skills whose name is neither in
///   the list nor already seeded once (`SKILLS_SEEDED_KEY`), so a skill
///   edited, disabled or deleted in the dashboard is never overwritten,
///   re-enabled or resurrected.
///
/// Idempotent and safe when several pods start at once: the first write is an
/// atomic insert-if-absent and later merges are compare-and-swap retried on
/// conflict, so a concurrent dashboard save or another pod's seed is never
/// clobbered. An unparseable stored list is left untouched.
pub fn seed_skills(
    db: &dyn crate::db::backend::DatabaseBackend,
    toml_skills: &[SkillDef],
) -> Result<SkillSeedOutcome> {
    use crate::db::settings::{SKILLS_KEY, SKILLS_SEEDED_KEY};
    const MAX_ATTEMPTS: usize = 8;

    // First occurrence wins on duplicate names in the TOML. Unknown pipeline
    // values are stripped here so a TOML typo can never land in the DB list
    // (where it would make every Settings save carry an invalid value).
    let sanitized: Vec<SkillDef> = toml_skills
        .iter()
        .cloned()
        .map(|mut s| {
            for w in sanitize_skill_pipelines(&mut s) {
                tracing::warn!("[[skills]] {w}");
            }
            s
        })
        .collect();
    let mut seeds: Vec<&SkillDef> = Vec::new();
    for s in &sanitized {
        if s.name.trim().is_empty() {
            tracing::warn!("[[skills]] entry with an empty name ignored");
            continue;
        }
        if seeds.iter().any(|x| x.name == s.name) {
            tracing::warn!(skill = %s.name, "duplicate [[skills]] name ignored");
            continue;
        }
        seeds.push(s);
    }

    let mut outcome = SkillSeedOutcome::default();
    let mut done = false;
    for _ in 0..MAX_ATTEMPTS {
        match db.get_app_setting(SKILLS_KEY)? {
            None => {
                let initial: Vec<SkillDef> = if seeds.is_empty() {
                    default_skills()
                } else {
                    seeds.iter().map(|s| (*s).clone()).collect()
                };
                let json = serde_json::to_string(&initial)?;
                if db.set_app_setting_if_absent(SKILLS_KEY, &json)? {
                    outcome.created = true;
                    outcome.added = initial.into_iter().map(|s| s.name).collect();
                    done = true;
                    break;
                }
                // Another pod (or the dashboard) won the race: merge into it.
            }
            Some(raw) => {
                let Ok(mut current) = serde_json::from_str::<Vec<SkillDef>>(&raw) else {
                    tracing::warn!("stored skills list is not valid JSON; TOML seeding skipped");
                    return Ok(outcome);
                };
                let seeded = load_seeded_names(db)?;
                let missing: Vec<SkillDef> = seeds
                    .iter()
                    .filter(|s| {
                        !current.iter().any(|c| c.name == s.name) && !seeded.contains(&s.name)
                    })
                    .map(|s| (*s).clone())
                    .collect();
                if missing.is_empty() {
                    done = true;
                    break;
                }
                let names: Vec<String> = missing.iter().map(|s| s.name.clone()).collect();
                current.extend(missing);
                let json = serde_json::to_string(&current)?;
                if db.compare_and_set_app_setting(SKILLS_KEY, &raw, &json)? {
                    outcome.added = names;
                    done = true;
                    break;
                }
                // Value changed under us (dashboard save / other pod): retry.
            }
        }
    }
    if !done {
        anyhow::bail!("skills seeding gave up after {MAX_ATTEMPTS} concurrent-update retries");
    }

    // Remember every TOML name now represented in the DB list, so deleting
    // one in the dashboard later is respected on the next start.
    if !seeds.is_empty() {
        for _ in 0..MAX_ATTEMPTS {
            let raw = db.get_app_setting(SKILLS_SEEDED_KEY)?;
            let mut names: Vec<String> = raw
                .as_deref()
                .and_then(|r| serde_json::from_str(r).ok())
                .unwrap_or_default();
            let before = names.len();
            for s in &seeds {
                if !names.contains(&s.name) {
                    names.push(s.name.clone());
                }
            }
            if names.len() == before && raw.is_some() {
                break;
            }
            let json = serde_json::to_string(&names)?;
            let ok = match raw.as_deref() {
                None => db.set_app_setting_if_absent(SKILLS_SEEDED_KEY, &json)?,
                Some(r) => db.compare_and_set_app_setting(SKILLS_SEEDED_KEY, r, &json)?,
            };
            if ok {
                break;
            }
        }
    }
    Ok(outcome)
}

fn load_seeded_names(db: &dyn crate::db::backend::DatabaseBackend) -> Result<Vec<String>> {
    Ok(db
        .get_app_setting(crate::db::settings::SKILLS_SEEDED_KEY)?
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default())
}

/// [`seed_skills`] for startup paths: logs instead of failing, so a DB hiccup
/// never blocks the daemon or a CLI command (pipelines still fall back to
/// [`load_skills`]'s defaults).
pub fn seed_skills_logged(db: &dyn crate::db::backend::DatabaseBackend, toml_skills: &[SkillDef]) {
    match seed_skills(db, toml_skills) {
        Ok(o) if !o.added.is_empty() => {
            tracing::info!(added = ?o.added, created = o.created, "skills seeded from config")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("skills seeding failed: {e:#}"),
    }
}

/// A "grand domain" — a broad area of the codebase/product (e.g. codex, bun,
/// c#). Multi-valued per PR/issue, assigned by the AI review, and surfaced as
/// filters in the PR network graph. Mirrors [`LabelDef`] minus the color/when.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DomainDef {
    /// Domain name (e.g. "codex", "bun", "c#").
    pub name: String,

    /// What this domain covers — helps the AI decide when it applies.
    #[serde(default)]
    pub description: Option<String>,

    /// Human-approved. Discovery + AI accumulation add domains as `false`
    /// (proposed); only **validated** domains are applied as `domain:<name>`
    /// GitHub labels. Gates writes to the repo behind a human check.
    #[serde(default)]
    pub validated: bool,
}

// ── Assign config ─────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct AssignConfig {
    #[serde(default)]
    pub enabled: bool,

    /// Assignees for issues (weighted random selection)
    #[serde(default)]
    pub issues: Vec<Assignee>,

    /// Assignees for PRs (weighted random selection)
    #[serde(default)]
    pub prs: Vec<Assignee>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Assignee {
    pub user: String,

    /// Weight for selection (higher = more likely). Doesn't need to sum to 100.
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_weight() -> u32 {
    1
}

impl AssignConfig {
    /// Pick an assignee from a list using weighted random selection.
    pub fn pick(assignees: &[Assignee]) -> Option<&str> {
        if assignees.is_empty() {
            return None;
        }
        let total: u32 = assignees.iter().map(|a| a.weight).sum();
        if total == 0 {
            return None;
        }
        let mut roll = rand_u32() % total;
        for a in assignees {
            if roll < a.weight {
                return Some(&a.user);
            }
            roll -= a.weight;
        }
        // Fallback (shouldn't happen)
        Some(&assignees[0].user)
    }
}

/// Simple random u32 without pulling in the `rand` crate.
/// Uses a counter to avoid identical results on rapid consecutive calls.
fn rand_u32() -> u32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    hasher.finish() as u32
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

    /// Override the AI system prompt for auto-fix.
    #[serde(default)]
    pub system_prompt: Option<String>,
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
            system_prompt: None,
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

    /// Days to keep rows in `change_events` (the issue/PR change history
    /// shown on `/history`). `0` disables the purge. Default: 90.
    #[serde(default = "default_history_retention_days")]
    pub history_retention_days: u32,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            bind: default_daemon_bind(),
            webhook_secret: None,
            apply: false,
            icm_enabled: false,
            icm_topic_prefix: default_icm_prefix(),
            history_retention_days: default_history_retention_days(),
        }
    }
}

fn default_history_retention_days() -> u32 {
    90
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebConfig {
    #[serde(default = "default_web_enabled")]
    pub enabled: bool,

    #[serde(default = "default_web_username")]
    pub username: String,

    #[serde(default)]
    pub password: Option<String>,

    /// Path to TLS certificate (PEM). Or use WSHM_TLS_CERT env.
    #[serde(default)]
    pub tls_cert: Option<String>,

    /// Path to TLS private key (PEM). Or use WSHM_TLS_KEY env.
    #[serde(default)]
    pub tls_key: Option<String>,
}

fn default_web_enabled() -> bool {
    false
}
fn default_web_username() -> String {
    "admin".to_string()
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_enabled(),
            username: default_web_username(),
            password: None,
            tls_cert: None,
            tls_key: None,
        }
    }
}

impl WebConfig {
    /// Resolve TLS paths: env > config.toml. Returns Some((cert, key)) if both set.
    pub fn resolve_tls(&self) -> Option<(String, String)> {
        let cert = std::env::var("WSHM_TLS_CERT")
            .ok()
            .or_else(|| self.tls_cert.clone())
            .filter(|s| !s.is_empty());
        let key = std::env::var("WSHM_TLS_KEY")
            .ok()
            .or_else(|| self.tls_key.clone())
            .filter(|s| !s.is_empty());
        match (cert, key) {
            (Some(c), Some(k)) => Some((c, k)),
            _ => None,
        }
    }

    /// Resolve the effective password: config.toml > $WSHM_WEB_PASSWORD > .wshm/credentials > auto-generate.
    /// If auto-generated, writes to .wshm/credentials and prints to stdout.
    pub fn resolve_password(&mut self, wshm_dir: &std::path::Path) {
        if self.password.is_some() {
            return;
        }

        if let Ok(env_pass) = std::env::var("WSHM_WEB_PASSWORD") {
            let env_pass = env_pass.trim();
            if !env_pass.is_empty() {
                self.password = Some(env_pass.to_string());
                return;
            }
        }

        let creds_path = wshm_dir.join("credentials");

        // Try reading WEB_PASSWORD from credentials file
        if let Ok(content) = std::fs::read_to_string(&creds_path) {
            for line in content.lines() {
                let line = line.trim();
                if let Some(val) = line.strip_prefix("WEB_PASSWORD=") {
                    let val = val.trim();
                    if !val.is_empty() {
                        self.password = Some(val.to_string());
                        return;
                    }
                }
            }
        }

        // Auto-generate
        let mut buf = [0u8; 16];
        if getrandom::getrandom(&mut buf).is_err() {
            return;
        }
        let generated: String = buf.iter().map(|b| format!("{:02x}", b)).collect();

        // Append to credentials file
        let entry = format!("WEB_PASSWORD={generated}\n");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&creds_path)
        {
            let _ = std::io::Write::write_all(&mut f, entry.as_bytes());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) =
                    std::fs::set_permissions(&creds_path, std::fs::Permissions::from_mode(0o600))
                {
                    tracing::warn!(
                        "Failed to set 0o600 on {}: {e}; web credentials may be world-readable",
                        creds_path.display()
                    );
                }
            }
        }

        eprintln!("Web UI credentials (auto-generated):");
        eprintln!("  Username: {}", self.username);
        eprintln!("  Password: {generated}");
        eprintln!("  Stored in: {}", creds_path.display());
        eprintln!();

        self.password = Some(generated);
    }
}

// ── Database backend config ──────────────────────────────────

/// Configuration for the database backend.
///
/// SQLite is the default (zero-config, single-file). PostgreSQL can be used
/// as an alternative for multi-repo setups or team environments.
///
/// ```toml
/// [database]
/// provider = "postgresql"          # "sqlite" (default) | "postgresql"
/// uri = "postgres://user:pass@host/db"
/// ```
#[derive(Debug, Deserialize, Serialize)]
pub struct DatabaseConfig {
    /// Database provider: "sqlite" (default) or "postgresql".
    #[serde(default = "default_db_provider")]
    pub provider: String,

    /// Connection URI for PostgreSQL (e.g. "postgres://user:pass@host/db").
    /// Ignored for SQLite. Can also be set via DATABASE_URL env var.
    #[serde(default)]
    pub uri: Option<String>,
}

fn default_db_provider() -> String {
    "sqlite".to_string()
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            provider: default_db_provider(),
            uri: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BrandingConfig {
    /// Bot display name in comments (default: "wshm")
    #[serde(default = "default_bot_name")]
    pub name: String,

    /// Bot URL linked in footers (default: "https://github.com/wshm-dev/wshm")
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

    /// Custom triage comment template (markdown/HTML).
    /// Placeholders: {category}, {priority}, {confidence}, {summary},
    /// {category_emoji}, {priority_emoji}, {relevant_files}, {duplicate_of},
    /// {header}, {footer}
    #[serde(default)]
    pub triage_template: Option<String>,

    /// Custom message shown when an issue is classified as a simple fix.
    /// Set to "" to hide the message entirely.
    #[serde(default)]
    pub simple_fix_message: Option<String>,

    /// Custom PR analysis comment template (markdown/HTML).
    /// Placeholders: {type}, {risk}, {summary}, {type_emoji}, {risk_emoji},
    /// {tests_present}, {breaking_change}, {docs_updated}, {linked_issues},
    /// {diagram} (architecture section, see `[pr.review_diagrams]`),
    /// {header}, {footer}
    #[serde(default)]
    pub pr_template: Option<String>,
}

impl Default for BrandingConfig {
    fn default() -> Self {
        Self {
            name: default_bot_name(),
            url: default_bot_url(),
            avatar_url: Some(
                "https://raw.githubusercontent.com/wshm-dev/wshm/main/assets/wizard-icon.png"
                    .to_string(),
            ),
            tagline: None,
            command_prefix: default_command_prefix(),
            footer_template: None,
            simple_fix_message: None,
            triage_template: None,
            pr_template: None,
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
            .unwrap_or("*{action} automatically by [{name}]({url})* · This is an automated analysis, not a human review.");

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
                format!("<img src=\"{avatar}\" width=\"48\" height=\"48\">")
            } else {
                tracing::warn!("Ignoring invalid avatar_url (must be HTTPS, no special chars)");
                "[w]".to_string()
            }
        } else {
            "[w]".to_string()
        };

        let tagline = self.tagline.as_deref().unwrap_or("Automated triage by AI");

        format!("> {icon} **{name}** · {tagline}\n\n", name = self.name,)
    }
}

fn default_bot_name() -> String {
    "wshm".to_string()
}
fn default_bot_url() -> String {
    "https://github.com/wshm-dev/wshm".to_string()
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

/// `[export.storage]`: same shape as every object storage target.
pub type StorageConfig = crate::storage::StorageTargetConfig;

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

// ── Notify config ─────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct NotifyConfig {
    /// Send notification at the end of `wshm run`
    #[serde(default)]
    pub on_run: bool,

    /// Discord webhook targets
    #[serde(default)]
    pub discord: Vec<DiscordNotifyConfig>,

    /// Slack webhook targets
    #[serde(default)]
    pub slack: Vec<SlackNotifyConfig>,

    /// Microsoft Teams webhook targets
    #[serde(default)]
    pub teams: Vec<TeamsNotifyConfig>,

    /// Generic webhook targets (raw JSON POST)
    #[serde(default)]
    pub webhooks: Vec<GenericNotifyWebhook>,
}

impl NotifyConfig {
    pub fn has_targets(&self) -> bool {
        !self.discord.is_empty()
            || !self.slack.is_empty()
            || !self.teams.is_empty()
            || !self.webhooks.is_empty()
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DiscordNotifyConfig {
    pub url: String,

    /// Optional username override for the bot
    #[serde(default)]
    pub username: Option<String>,

    /// Optional avatar URL override
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SlackNotifyConfig {
    pub url: String,

    /// Optional channel override (webhook default is used if absent)
    #[serde(default)]
    pub channel: Option<String>,

    /// Optional username override
    #[serde(default)]
    pub username: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TeamsNotifyConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GenericNotifyWebhook {
    pub url: String,

    /// Optional HMAC-SHA256 secret
    #[serde(default)]
    pub secret: Option<String>,
}

// ── License config ────────────────────────────────────────────

/// License key resolution configuration.
///
/// Resolution order:
/// 1. Vault placeholder in `key` field (e.g. `vault(secret/wshm/license)`)
/// 2. Environment variable `WSHM_LICENSE_KEY`
/// 3. File path in `path` field (JWT file)
/// 4. Fallback: `~/.wshm/license.jwt`
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct LicenseConfig {
    /// License key or vault placeholder.
    /// Examples:
    ///   - `vault(secret/wshm/license-key)` (HashiCorp)
    ///   - `vault(wshm/license-key)` (AWS Secrets Manager)
    ///   - `vault(wshm-keyvault/license-key)` (Azure Key Vault)
    ///   - `vault(projects/my-proj/secrets/wshm-license)` (GCP)
    #[serde(default)]
    pub key: Option<String>,

    /// Path to a cached JWT license file.
    /// Default: `~/.wshm/license.jwt`
    #[serde(default)]
    pub path: Option<String>,
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
    "127.0.0.1:3000".to_string()
}

fn default_icm_prefix() -> String {
    "wshm".to_string()
}

// ── Global multi-repo config ──────────────────────────────────

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub daemon: GlobalDaemonConfig,

    #[serde(default)]
    pub ai: Option<AiConfig>,

    #[serde(default)]
    pub update: UpdateConfig,

    /// Transient-HTTP-error retry policy, shared by every outbound HTTP
    /// call (poller, git providers, AI, self-update). Editable live from
    /// Settings -> Reliability.
    #[serde(default)]
    pub retry: crate::retry::RetryConfig,

    #[serde(default)]
    pub repos: Vec<RepoEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
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

    /// Multi-pod: how often (seconds) each pod re-reads the persisted repo
    /// registry (features, apply mode, added/removed repos) so a change made
    /// through one pod reaches the others without a restart. Only active when
    /// the persistence layer is shared (Pro on Postgres). `0` disables it.
    #[serde(default = "default_registry_sync_secs")]
    pub registry_sync_secs: u64,
}

impl Default for GlobalDaemonConfig {
    fn default() -> Self {
        Self {
            bind: default_daemon_bind(),
            webhook_secret: None,
            apply: false,
            poll: false,
            poll_interval: default_poll_interval(),
            registry_sync_secs: default_registry_sync_secs(),
        }
    }
}

fn default_registry_sync_secs() -> u64 {
    30
}

fn default_poll_interval() -> u64 {
    30
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct RepoEntry {
    /// Repository slug: "owner/repo"
    pub slug: String,

    /// Absolute path to the local checkout
    pub path: PathBuf,

    /// Per-repo apply override (inherits from [daemon].apply if not set)
    #[serde(default)]
    pub apply: Option<bool>,

    /// Enable/disable this repo (default: true)
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Per-repo webhook secret override
    #[serde(default)]
    pub secret: Option<String>,

    /// Per-repo feature toggles. When the section is absent in the TOML,
    /// `RepoFeatures::default()` is used — read-only collection of issues
    /// and PRs only. Mutating actions (triage / analyze / auto_pr / merge)
    /// are opt-in.
    #[serde(default)]
    pub features: RepoFeatures,
}

/// Feature flags controlling what wshm does per repository. Defaults are
/// conservative: read-only collection enabled, every mutating action
/// disabled. The legacy `apply: true` flag in `[[repos]]` upgrades the
/// triage/analyze/auto_pr trio at config-load time so existing setups
/// keep behaving the same.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RepoFeatures {
    // Read-only collection
    #[serde(default = "default_true")]
    pub collect_issues: bool,
    #[serde(default = "default_true")]
    pub collect_prs: bool,

    // AI-assisted classification
    #[serde(default)]
    pub triage_issues: bool,
    #[serde(default)]
    pub analyze_prs: bool,
    /// Append the "Related history" block (closed issues / merged PRs found
    /// by full-text search, see `ai::related`) to this repo's triage and
    /// PR-analysis prompts. Per-repo switch on top of the global
    /// `[ai.rag] enabled` / `WSHM_RAG_ENABLED`; both must be on.
    #[serde(default = "default_true")]
    pub related_history: bool,
    /// Pro-only inline code review.
    #[serde(default)]
    pub review_prs: bool,
    /// Whether a computed review is also posted to GitHub as inline PR
    /// comments. Independent of `apply` (the repo's general "write to
    /// GitHub" mode) so a repo can run review computation silently — shown
    /// on the dashboard only — even while `apply` is on for other features.
    /// Defaults true so enabling `review_prs` elsewhere keeps today's
    /// behavior (compute-and-post) unless explicitly turned off.
    #[serde(default = "default_true")]
    pub review_post_comments: bool,

    // Mutating actions on the repo
    #[serde(default)]
    pub auto_pr: bool,
    #[serde(default)]
    pub auto_merge: bool,

    /// Fine-grained filters applied per action.
    #[serde(default)]
    pub filters: RepoFilters,
}

impl Default for RepoFeatures {
    fn default() -> Self {
        Self {
            collect_issues: true,
            collect_prs: true,
            triage_issues: false,
            analyze_prs: false,
            related_history: true,
            review_prs: false,
            review_post_comments: true,
            auto_pr: false,
            auto_merge: false,
            filters: RepoFilters::default(),
        }
    }
}

impl RepoFeatures {
    /// Upgrade defaults when the legacy `apply: true` flag is set, so
    /// existing repos keep behaving the same after this migration. Only
    /// flips off→on; never overrides explicit user choices.
    pub fn merge_legacy_apply(&mut self, apply: bool) {
        if apply {
            self.triage_issues = true;
            self.analyze_prs = true;
            self.auto_pr = true;
        }
    }
}

/// Per-action filters (whitelist + blacklist combinable). Empty vector
/// or `0` numeric = no filter on that dimension. All fields default to
/// permissive — restrict explicitly when narrower behavior is wanted.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RepoFilters {
    // ── Global (apply to every action) ────────────────────────────
    #[serde(default)]
    pub skip_authors: Vec<String>,
    #[serde(default)]
    pub target_branches: Vec<String>,
    #[serde(default)]
    pub skip_drafts: bool,

    // ── Triage (issues) ───────────────────────────────────────────
    #[serde(default)]
    pub triage_only_labels: Vec<String>,
    #[serde(default)]
    pub triage_skip_labels: Vec<String>,
    #[serde(default)]
    pub triage_max_age_days: u32,
    /// Labels that, when present on an open issue, force re-triage even
    /// when the content hash is unchanged. After a successful applied
    /// triage the bot removes these labels so they don't re-fire forever.
    #[serde(default = "default_triage_relabel_labels")]
    pub triage_relabel_labels: Vec<String>,
    /// Re-triage an open issue that has zero labels if its last triage is
    /// older than this many hours. `0` disables the trigger.
    #[serde(default = "default_triage_no_labels_min_age_hours")]
    pub triage_no_labels_min_age_hours: u32,
    /// Label-name prefixes that wshm treats as "already triaged" evidence
    /// on the forge. When an open issue carries any label whose name starts
    /// with one of these prefixes and has no local `triage_results` row,
    /// the pipeline seeds a stub row from the labels instead of spending
    /// an LLM call. Empty disables the safety net (legacy behaviour).
    #[serde(default = "default_triage_managed_label_prefixes")]
    pub triage_managed_label_prefixes: Vec<String>,
    /// Grace window before label-based seeding kicks in. An open issue
    /// must have been quiet (no `updated_at` change) for at least this
    /// many hours before its labels are trusted as evidence of a prior
    /// triage. `0` (default) means seed immediately.
    #[serde(default)]
    pub triage_managed_label_grace_hours: u32,

    // ── Analyze PRs ───────────────────────────────────────────────
    #[serde(default)]
    pub analyze_min_loc: u32,
    #[serde(default)]
    pub analyze_max_loc: u32,

    // ── Auto-fix PR ───────────────────────────────────────────────
    #[serde(default)]
    pub auto_pr_only_labels: Vec<String>,
    #[serde(default)]
    pub auto_pr_target_branch: String,

    // ── Auto-merge ────────────────────────────────────────────────
    #[serde(default)]
    pub auto_merge_only_authors: Vec<String>,
    #[serde(default)]
    pub auto_merge_only_labels: Vec<String>,
    #[serde(default)]
    pub auto_merge_min_approvals: u32,
    #[serde(default)]
    pub auto_merge_max_loc: u32,
}

fn default_triage_relabel_labels() -> Vec<String> {
    vec!["wshm:relabel".to_string()]
}

fn default_triage_no_labels_min_age_hours() -> u32 {
    24
}

fn default_triage_managed_label_prefixes() -> Vec<String> {
    vec!["priority:".to_string(), "category:".to_string()]
}

impl Default for RepoFilters {
    fn default() -> Self {
        Self {
            skip_authors: Vec::new(),
            target_branches: Vec::new(),
            skip_drafts: false,
            triage_only_labels: Vec::new(),
            triage_skip_labels: Vec::new(),
            triage_max_age_days: 0,
            triage_relabel_labels: default_triage_relabel_labels(),
            triage_no_labels_min_age_hours: default_triage_no_labels_min_age_hours(),
            triage_managed_label_prefixes: default_triage_managed_label_prefixes(),
            triage_managed_label_grace_hours: 0,
            analyze_min_loc: 0,
            analyze_max_loc: 0,
            auto_pr_only_labels: Vec::new(),
            auto_pr_target_branch: String::new(),
            auto_merge_only_authors: Vec::new(),
            auto_merge_only_labels: Vec::new(),
            auto_merge_min_approvals: 0,
            auto_merge_max_loc: 0,
        }
    }
}

impl RepoFilters {
    pub fn is_author_skipped(&self, author: Option<&str>) -> bool {
        let a = match author {
            Some(s) => s,
            None => return false,
        };
        self.skip_authors.iter().any(|x| x == a)
    }

    pub fn branch_allowed(&self, base_branch: Option<&str>) -> bool {
        if self.target_branches.is_empty() {
            return true;
        }
        match base_branch {
            Some(b) => self.target_branches.iter().any(|x| x == b),
            None => false,
        }
    }

    pub fn issue_labels_pass_triage(&self, labels: &[String]) -> bool {
        if !self.triage_only_labels.is_empty()
            && !labels.iter().any(|l| self.triage_only_labels.contains(l))
        {
            return false;
        }
        if labels.iter().any(|l| self.triage_skip_labels.contains(l)) {
            return false;
        }
        true
    }

    pub fn issue_age_ok(&self, created_at: &str) -> bool {
        if self.triage_max_age_days == 0 {
            return true;
        }
        match created_at.parse::<chrono::DateTime<chrono::Utc>>() {
            Ok(dt) => {
                let age = chrono::Utc::now().signed_duration_since(dt);
                age.num_days() <= self.triage_max_age_days as i64
            }
            Err(_) => true,
        }
    }
}

fn default_enabled() -> bool {
    true
}

impl GlobalConfig {
    pub fn load(path: &Path) -> Result<Self> {
        // If the file doesn't exist yet, start with an empty multi-repo config.
        // This enables a "first-run" UX where the user adds repos through the
        // web UI's POST /api/v1/repos endpoint.
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read global config: {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse global config: {}", path.display()))?;
        // Empty `repos` is allowed — the user can add them at runtime.
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self).context("Failed to serialize global config")?;
        fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// Default global config path: ~/.wshm/global.toml
    pub fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".wshm")
            .join("global.toml")
    }
}

/// Append a new repo entry to the global config TOML and save back.
/// Used by the dynamic add_repo flow so a runtime addition survives restart.
pub fn append_repo_to_global(
    path: &Path,
    slug: &str,
    repo_path: &Path,
    apply: Option<bool>,
) -> Result<()> {
    let mut global = GlobalConfig::load(path)
        .with_context(|| format!("Failed to load global config at {}", path.display()))?;
    if global.repos.iter().any(|r| r.slug == slug) {
        anyhow::bail!("repo {slug} already present in global config");
    }
    global.repos.push(RepoEntry {
        slug: slug.to_string(),
        path: repo_path.to_path_buf(),
        apply,
        enabled: true,
        secret: None,
        features: RepoFeatures::default(),
    });
    global.save(path)
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

        config.ai.rag.apply_env_overrides();
        Ok(config)
    }

    pub fn fix_secret_env_vars(&self) -> Vec<String> {
        self.fix.secret_env.clone()
    }

    /// Filter labels: remove blacklisted ones. If [[labels]] allowlist is configured,
    /// only allow labels that match a defined name.
    pub fn filter_labels(&self, labels: Vec<String>) -> Vec<String> {
        let mut filtered: Vec<String> = labels
            .into_iter()
            .filter(|l| {
                let normalized = l.replace('_', " ").to_lowercase();
                !self.labels_blacklist.iter().any(|b| {
                    let b_normalized = b.replace('_', " ").to_lowercase();
                    b_normalized == normalized
                })
            })
            .collect();

        // If label definitions exist, enforce allowlist
        if !self.labels.is_empty() {
            filtered.retain(|l| {
                self.labels
                    .iter()
                    .any(|def| def.name.eq_ignore_ascii_case(l))
            });
        }

        filtered
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
            // Try to detect from git remote first
            match detect_repo() {
                Ok((owner, name)) => {
                    config.repo_owner = owner;
                    config.repo_name = name;
                }
                Err(_) => {
                    // Not in a git repo and no --repo flag: try global.toml fallback
                    let global_path = GlobalConfig::default_path();
                    if global_path.exists() {
                        let global = GlobalConfig::load(&global_path)
                            .context("Failed to load ~/.wshm/global.toml")?;

                        let enabled_repos: Vec<_> =
                            global.repos.iter().filter(|r| r.enabled).collect();

                        if enabled_repos.len() == 1 {
                            // Exactly one repo: use it automatically
                            let repo = enabled_repos[0];
                            let parts: Vec<&str> = repo.slug.splitn(2, '/').collect();
                            if parts.len() != 2 {
                                anyhow::bail!(
                                    "Invalid repo slug in global.toml: {} (expected owner/repo)",
                                    repo.slug
                                );
                            }
                            config.repo_owner = parts[0].to_string();
                            config.repo_name = parts[1].to_string();
                        } else if enabled_repos.is_empty() {
                            anyhow::bail!("Not inside a git repository and no enabled repos in ~/.wshm/global.toml. Use --repo owner/repo");
                        } else {
                            eprintln!(
                                "Found ~/.wshm/global.toml with {} repos:",
                                enabled_repos.len()
                            );
                            for (i, repo) in enabled_repos.iter().enumerate() {
                                eprintln!("  {}. {}", i + 1, repo.slug);
                            }
                            eprintln!();
                            eprintln!(
                                "Use --repo <slug> to select, or run all with: wshm run --all"
                            );
                            anyhow::bail!("Ambiguous repo selection");
                        }
                    } else {
                        // No global.toml: show original error
                        anyhow::bail!("Not inside a git repository. Use --repo owner/repo");
                    }
                }
            }
        }

        config.ai.rag.apply_env_overrides();
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
#
# Alternative: authenticate as a GitHub App instead of a personal token.
# A self-refreshing installation token beats a PAT for an unattended bot —
# no manual rotation, and it's scoped to only the repos the App is
# installed on. Set all three (env or Settings -> Secrets), never in this
# file: WSHM_GITHUB_APP_ID, WSHM_GITHUB_APP_INSTALLATION_ID,
# WSHM_GITHUB_APP_PRIVATE_KEY (PEM; literal "\n" line breaks are accepted).
# When present, App auth takes priority over GITHUB_TOKEN/WSHM_TOKEN.

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
# suggested_actions = true        # append "Suggested Actions" to triage comments (set false to disable)

[pr]
enabled = true
auto_label = true
risk_labels = true
# model = "claude-sonnet-4-20250514"    # override: default model for PR analysis

# Architecture diagram in the PR analysis comment (one extra AI call per new
# diff, cached per head). Every key can be overridden per repo in
# Settings → Diagrams (stored in the DB; DB > this file > defaults).
# [pr.review_diagrams]
# enabled = true
# kinds = ["architecture"]       # only "architecture" for now
# text = "short"                 # "short" | "normal" | "full"
# language = "en"                # language of the diagram texts
# comment = "mermaid"            # "mermaid" (diagram in the comment) | "none" (link only)
# diff_viewer = true             # embed the diff in the interactive page
# active_days = 14               # skip PRs idle for longer (0 = no limit)
# max_mermaid_chars = 5000       # Mermaid size cap (Forgejo/Gitea default limit)
# timeout_secs = 120             # budget of the background diagram step
# max_summary_calls = 3          # per-file summary calls for diffs > 40 KB
#
# Interactive page (React Flow viewer). Off by default.
# [pr.review_diagrams.publish]
# target = "off"                 # "off" | "local" | "github-pages" | "wshm" | "s3" | "gcs" | "azure"
# prefix = "wshm-diagrams"
# public_url = "https://diagrams.example.com"   # link = public_url/prefix/...
# dir = "/var/www/diagrams"      # local (TOML only)
# pages_repo = "owner/diagrams"  # github-pages, gh-pages branch, repo token (TOML only).
#                                # If that repo is public, its tree and git history
#                                # list every page: the token in the file name does
#                                # not prevent enumeration there.
# retention_days = 30            # delete pages idle for longer (0 = until the PR closes)
# max_pages = 200                # per repo, least recently updated evicted first
# max_diff_kb = 1024             # bigger diffs: the viewer links to "Files changed"
# allow_private = false          # allow public targets for a private repo (TOML only)

[queue]
enabled = true
merge_threshold = 15
strategy = "rebase"

# Tunable weights for the merge-queue + issue ranking algorithms.
# Defaults below are the in-code defaults; uncomment to override.
# [scoring.pr]
# ci_green = 25
# ci_failure = -30
# ci_pending = -5
# mergeable = 2
# conflict = -25
# linked_issue = 5
# label_priority_critical = 30
# label_priority_high = 15
# label_priority_medium = 5
# label_bug = 5
# label_blocked = -50
# age_bonus_max = 10
# age_decay_after_days = 60
# age_decay_penalty = -15
# momentum_recent = 5
# momentum_stale = -10
# recent_days = 1
# stale_days = 30
#
# [scoring.issue]
# ai_priority_critical = 50
# ai_priority_high = 25
# ai_priority_medium = 10
# low_confidence_threshold = 0.5
# reaction_per_plus1 = 1
# reaction_max_bonus = 20
# label_security = 30
# label_regression = 15
# label_bug = 8
# label_first_issue = 3
# label_blocked = -10
# recent_days = 7
# recent_bonus = 5
# stale_days = 90
# stale_penalty = -10

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

# Labels that wshm must never apply (case-insensitive)
# labels_blacklist = ["do-not-touch", "manual-only", "security"]

# Issues/PRs that wshm must never touch
# issues_blacklist = [755, 123]
# prs_blacklist = [42]

# Label definitions (fed to AI for better label selection)
# [[labels]]
# name = "bug"
# description = "Something is broken or produces wrong results"
# when = "Confirmed broken behavior with clear repro steps"
#
# [[labels]]
# name = "enhancement"
# description = "New feature or improvement request"
# when = "Request for new capability, not a fix"
#
# [[labels]]
# name = "priority:critical"
# description = "Blocks users, data loss, security vulnerability"
# when = "Production down, data corruption, or security exploit"
#
# [[labels]]
# name = "priority:high"
# description = "Major impact, needs attention soon"
# when = "Core feature broken, no workaround"
#
# [[labels]]
# name = "priority:medium"
# description = "Moderate impact, can wait"
# when = "Feature degraded but workaround exists"
#
# [[labels]]
# name = "priority:low"
# description = "Minor or cosmetic"
# when = "Edge case or low-impact improvement"

# Skills: standing instructions appended to AI prompts. This is only the
# SEED: at startup, skills are copied into the database (once per name) and
# from then on are edited in Settings -> Skills. Editing/removing an entry
# here never overwrites what was changed in the dashboard.
# pipelines: any of "triage", "pr_review", "review", "diagrams" (empty = all)
# [[skills]]
# name = "rust-error-handling"
# description = "Shown in Settings only, not sent to the AI"
# content = "Flag unwrap()/expect() on fallible paths in non-test code."
# pipelines = ["pr_review", "review"]
# enabled = true

# [assign]
# enabled = true
#
# [[assign.issues]]
# user = "alice"
# weight = 70
#
# [[assign.issues]]
# user = "bob"
# weight = 30
#
# [[assign.prs]]
# user = "alice"
# weight = 50
#
# [[assign.prs]]
# user = "bob"
# weight = 50

# [update]
# enabled = false                        # Enable automatic update checks in daemon mode
# interval_hours = 6                     # Check interval
# auto_apply = false                     # Auto-install updates (daemon uses true)

# [branding]
# name = "wshm"                       # Bot display name in comments
# url = "https://github.com/wshm-dev/wshm"        # Link in comment footers
# avatar_url = "https://example.com/logo.png"   # Optional avatar in headers
# tagline = "AI-powered repo assistant"          # Optional tagline
# command_prefix = "/wshm"             # Slash command prefix
# footer_template = "*{action} by [{name}]({url})*"  # Custom footer
#
# Custom comment templates (markdown/HTML). Omit to use defaults.
# Triage placeholders: {header}, {footer}, {category}, {priority}, {confidence},
#   {summary}, {category_emoji}, {priority_emoji}, {relevant_files}, {duplicate_of}
# triage_template = """
# {header}
# ## Triage Result
# **{category_emoji} {category}** — {summary}
# {footer}
# """
#
# PR analysis placeholders: {header}, {footer}, {type}, {risk}, {summary},
#   {type_emoji}, {risk_emoji}, {tests_present}, {breaking_change},
#   {docs_updated}, {linked_issues}, {diagram}
# pr_template = """
# {header}
# ## PR Review
# **{type_emoji} {type}** | Risk: {risk_emoji} {risk}
# {summary}
# {footer}
# """

# [web]
# enabled = true                        # Enable web dashboard on daemon port
# username = "admin"                    # Basic auth username
# password = "changeme"                # Basic auth password (or auto-generated in .wshm/credentials)

# [license]
# Resolution order: vault → env WSHM_LICENSE_KEY → path → ~/.wshm/license.jwt
#
# key = "vault(secret/wshm/license-key)"               # HashiCorp Vault
# key = "vault(wshm/license-key)"                       # AWS Secrets Manager
# key = "vault(wshm-keyvault/license-key)"              # Azure Key Vault
# key = "vault(projects/my-proj/secrets/wshm-license)"  # GCP Secret Manager
#
# path = "~/.wshm/license.jwt"         # Explicit path to cached JWT
#
# Or use env var: WSHM_LICENSE_KEY=WSHM-XXXX-XXXX-XXXX

# [vault]
# provider = "hashicorp"               # "hashicorp" | "aws" | "gcp"
# address = "https://vault.example.com"
# mount = "secret"
# Auth from env: VAULT_TOKEN, VAULT_ROLE_ID, etc.

# [notify]
# on_run = true                         # Send summary after `wshm run`
#
# [[notify.discord]]
# url = "https://discord.com/api/webhooks/YOUR_ID/YOUR_TOKEN"
# username = "wshm"
# avatar_url = "https://github.com/wshm-dev.png"
#
# [[notify.slack]]
# url = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"
# channel = "repo-updates"
# username = "wshm"
#
# [[notify.teams]]
# url = "https://outlook.office.com/webhook/YOUR/WEBHOOK/URL"
#
# [[notify.webhooks]]
# url = "https://your-server.com/wshm-notify"
# secret = "hmac-secret"

# [export.storage]
# provider = "s3"                      # "s3" | "gcs"
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
        self.github_token_optional()
            .context("No GitHub token found. Set GITHUB_TOKEN, WSHM_TOKEN, authenticate with `gh auth login`, or add a github_token in Settings → Secrets")
    }

    /// Resolve GitHub App credentials for self-refreshing bot auth, without
    /// bailing when absent (App auth is opt-in; most installs still use a
    /// PAT). All three fields must be present and non-empty — a partially
    /// configured App is treated as "not configured" so callers fall back
    /// to [`Self::github_token_optional`] instead of failing outright.
    ///
    /// Same resolution order as `github_token_optional`: encrypted secret
    /// store (per-repo override → global) → env var.
    pub fn github_app_auth_optional(&self) -> Option<GithubAppAuth> {
        let store = crate::secrets::global();
        let slug = self.repo_slug();
        let resolve = |key: &str, env_name: &str| {
            crate::secrets::resolve(store.as_ref(), Some(&slug), key, env_name)
                .filter(|v| !v.trim().is_empty())
        };

        let app_id = resolve("github_app_id", "WSHM_GITHUB_APP_ID")?;
        let installation_id = resolve(
            "github_app_installation_id",
            "WSHM_GITHUB_APP_INSTALLATION_ID",
        )?;
        let private_key_pem = resolve("github_app_private_key", "WSHM_GITHUB_APP_PRIVATE_KEY")?;

        let app_id = app_id.trim().parse().ok().or_else(|| {
            tracing::warn!("github_app_id is not a valid integer — ignoring GitHub App auth");
            None
        })?;
        let installation_id = installation_id.trim().parse().ok().or_else(|| {
            tracing::warn!(
                "github_app_installation_id is not a valid integer — ignoring GitHub App auth"
            );
            None
        })?;

        // A PEM pasted into a single-line secret input can't carry real
        // newlines, so accept the literal two-character sequence `\n` as an
        // escape and unescape it here — same convention as Firebase/Vercel
        // service-account env vars.
        let private_key_pem = private_key_pem.replace("\\n", "\n");

        Some(GithubAppAuth {
            app_id,
            installation_id,
            private_key_pem,
        })
    }

    /// Resolve a GitHub token without bailing when absent. Returns `None` so
    /// the daemon can run in anonymous read-only mode against public repos
    /// (rate-limited to 60 req/h). Mutating endpoints (post labels/comments,
    /// create PRs) must check token presence before calling.
    pub fn github_token_optional(&self) -> Option<String> {
        // 1. Encrypted secret store (per-repo override → global → env fallback).
        if let Some(store) = crate::secrets::global() {
            if let Some(v) = crate::secrets::resolve(
                Some(&store),
                Some(&self.repo_slug()),
                "github_token",
                "GITHUB_TOKEN",
            ) {
                if !v.trim().is_empty() {
                    return Some(v);
                }
            }
        }
        // 2. Legacy env vars + gh CLI fallback (kept for OSS standalone).
        let token = std::env::var("WSHM_TOKEN")
            .ok()
            .or_else(|| std::env::var("GITHUB_TOKEN").ok())
            .or_else(|| gh_auth_token().ok())?;
        if token.trim().is_empty() {
            return None;
        }
        Some(token)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            github: GitHubConfig::default(),
            git_provider: None,
            git_url: None,
            azure_devops: AzureDevOpsConfig::default(),
            ai: AiConfig::default(),
            triage: TriageConfig::default(),
            pr: PrConfig::default(),
            queue: QueueConfig::default(),
            scoring: ScoringConfig::default(),
            conflicts: ConflictConfig::default(),
            sync: SyncConfig::default(),
            fix: FixConfig::default(),
            assign: AssignConfig::default(),
            daemon: DaemonConfig::default(),
            branding: BrandingConfig::default(),
            update: UpdateConfig::default(),
            export: ExportConfig::default(),
            notify: NotifyConfig::default(),
            vault: None,
            license: LicenseConfig::default(),
            web: WebConfig::default(),
            database: None,
            labels_blacklist: Vec::new(),
            issues_blacklist: Vec::new(),
            prs_blacklist: Vec::new(),
            labels: Vec::new(),
            skills: Vec::new(),
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

    /// Self-contained: sets/clears its own env vars sequentially so it's
    /// safe alongside other tests running in the same process (no other
    /// test touches these var names). `secrets::global()` is never
    /// installed in a unit-test process, so `github_app_auth_optional`
    /// resolves purely from env — matching production's env-var fallback
    /// path for a standalone (non-daemon) install.
    #[test]
    fn test_github_app_auth_optional_requires_all_three_fields() {
        const VARS: [&str; 3] = [
            "WSHM_GITHUB_APP_ID",
            "WSHM_GITHUB_APP_INSTALLATION_ID",
            "WSHM_GITHUB_APP_PRIVATE_KEY",
        ];
        for v in VARS {
            std::env::remove_var(v);
        }

        let config = Config {
            repo_owner: "acme".to_string(),
            repo_name: "widgets".to_string(),
            ..Config::default()
        };

        assert!(
            config.github_app_auth_optional().is_none(),
            "no App fields set — must not report configured"
        );

        std::env::set_var("WSHM_GITHUB_APP_ID", "123456");
        assert!(
            config.github_app_auth_optional().is_none(),
            "only app_id set — partial config must still be None"
        );

        std::env::set_var("WSHM_GITHUB_APP_INSTALLATION_ID", "789");
        std::env::set_var(
            "WSHM_GITHUB_APP_PRIVATE_KEY",
            "-----BEGIN RSA PRIVATE KEY-----\\nabc\\n-----END RSA PRIVATE KEY-----",
        );
        let auth = config
            .github_app_auth_optional()
            .expect("all three fields set — must resolve");
        assert_eq!(auth.app_id, 123456);
        assert_eq!(auth.installation_id, 789);
        assert_eq!(
            auth.private_key_pem,
            "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----",
            "literal \\n must be unescaped to real newlines"
        );

        for v in VARS {
            std::env::remove_var(v);
        }
    }

    fn skill(name: &str, pipelines: &[&str], enabled: bool) -> SkillDef {
        SkillDef {
            name: name.to_string(),
            description: None,
            content: format!("do {name} things"),
            pipelines: pipelines.iter().map(|s| s.to_string()).collect(),
            enabled,
        }
    }

    #[test]
    fn test_skills_prompt_filters_by_pipeline_and_enabled() {
        let skills = vec![
            skill("triage-only", &["triage"], true),
            skill("pr-only", &["pr_review"], true),
            skill("both", &[], true),
            skill("disabled", &["triage"], false),
        ];

        let triage = skills_prompt(&skills, "triage");
        assert!(triage.contains("triage-only"));
        assert!(triage.contains("both"));
        assert!(!triage.contains("pr-only"));
        assert!(!triage.contains("disabled"));

        let pr = skills_prompt(&skills, "pr_review");
        assert!(pr.contains("pr-only"));
        assert!(pr.contains("both"));
        assert!(!pr.contains("triage-only"));

        assert_eq!(skills_prompt(&[], "triage"), "");
    }

    #[test]
    fn test_skills_prompt_diagrams_pipeline() {
        let skills = vec![skill("diag", &["diagrams"], true), skill("all", &[], true)];
        let d = skills_prompt(&skills, "diagrams");
        assert!(d.contains("diag") && d.contains("all"));
        assert!(!skills_prompt(&skills, "triage").contains("### diag"));
        assert!(SKILL_PIPELINES.contains(&"diagrams"));
    }

    #[test]
    fn test_toml_skills_parse() {
        let cfg: Config = toml::from_str(
            r#"
[[skills]]
name = "a"
content = "do a"
pipelines = ["diagrams"]

[[skills]]
name = "b"
content = "do b"
enabled = false
"#,
        )
        .unwrap();
        assert_eq!(cfg.skills.len(), 2);
        assert_eq!(cfg.skills[0].pipelines, vec!["diagrams"]);
        assert!(cfg.skills[0].enabled, "enabled defaults to true");
        assert!(!cfg.skills[1].enabled);
        assert!(toml::from_str::<Config>("").unwrap().skills.is_empty());
    }

    fn stored(db: &crate::db::Database) -> Vec<SkillDef> {
        serde_json::from_str(
            &db.get_app_setting(crate::db::settings::SKILLS_KEY)
                .unwrap()
                .unwrap(),
        )
        .unwrap()
    }

    fn names(v: &[SkillDef]) -> Vec<&str> {
        v.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn test_seed_fresh_writes_toml_skills_only() {
        let db = crate::db::Database::open_memory().unwrap();
        let o = seed_skills(&db, &[skill("a", &[], true), skill("b", &[], true)]).unwrap();
        assert!(o.created);
        assert_eq!(o.added, vec!["a", "b"]);
        assert_eq!(
            names(&stored(&db)),
            vec!["a", "b"],
            "no default when TOML defines skills"
        );
    }

    #[test]
    fn test_seed_fresh_without_toml_writes_defaults() {
        let db = crate::db::Database::open_memory().unwrap();
        let o = seed_skills(&db, &[]).unwrap();
        assert!(o.created);
        assert_eq!(names(&stored(&db)), names(&default_skills()));
        // Idempotent.
        assert_eq!(seed_skills(&db, &[]).unwrap(), SkillSeedOutcome::default());
    }

    #[test]
    fn test_seed_is_idempotent() {
        let db = crate::db::Database::open_memory().unwrap();
        let toml = [skill("a", &[], true)];
        seed_skills(&db, &toml).unwrap();
        let before = db.get_app_setting(crate::db::settings::SKILLS_KEY).unwrap();
        let o = seed_skills(&db, &toml).unwrap();
        assert_eq!(o, SkillSeedOutcome::default());
        assert_eq!(
            db.get_app_setting(crate::db::settings::SKILLS_KEY).unwrap(),
            before
        );
    }

    #[test]
    fn test_seed_never_overwrites_or_reenables_dashboard_edits() {
        let db = crate::db::Database::open_memory().unwrap();
        seed_skills(&db, &[skill("a", &[], true)]).unwrap();
        // Dashboard edit: disable + rewrite "a".
        let mut edited = skill("a", &["triage"], false);
        edited.content = "user text".into();
        db.set_app_setting(
            crate::db::settings::SKILLS_KEY,
            &serde_json::to_string(&[edited.clone()]).unwrap(),
        )
        .unwrap();
        // TOML still has the original, plus a new skill "c".
        let o = seed_skills(&db, &[skill("a", &[], true), skill("c", &[], true)]).unwrap();
        assert!(!o.created);
        assert_eq!(o.added, vec!["c"]);
        let s = stored(&db);
        assert_eq!(names(&s), vec!["a", "c"]);
        assert_eq!(s[0], edited, "edited/disabled skill untouched");
    }

    #[test]
    fn test_seed_existing_list_adds_missing_without_defaults() {
        let db = crate::db::Database::open_memory().unwrap();
        // Pre-existing list saved before TOML seeding existed (even empty).
        db.set_app_setting(crate::db::settings::SKILLS_KEY, "[]")
            .unwrap();
        let o = seed_skills(&db, &[skill("a", &[], true)]).unwrap();
        assert_eq!(o.added, vec!["a"]);
        assert_eq!(names(&stored(&db)), vec!["a"]);
        // Existing empty list + no TOML: defaults NOT re-added.
        let db2 = crate::db::Database::open_memory().unwrap();
        db2.set_app_setting(crate::db::settings::SKILLS_KEY, "[]")
            .unwrap();
        seed_skills(&db2, &[]).unwrap();
        assert!(stored(&db2).is_empty());
    }

    #[test]
    fn test_seed_respects_dashboard_deletion() {
        let db = crate::db::Database::open_memory().unwrap();
        let toml = [skill("a", &[], true), skill("b", &[], true)];
        seed_skills(&db, &toml).unwrap();
        // User deletes "a" in the dashboard.
        db.set_app_setting(
            crate::db::settings::SKILLS_KEY,
            &serde_json::to_string(&[skill("b", &[], true)]).unwrap(),
        )
        .unwrap();
        let o = seed_skills(&db, &toml).unwrap();
        assert!(o.added.is_empty(), "deleted TOML skill must not come back");
        assert_eq!(names(&stored(&db)), vec!["b"]);
    }

    #[test]
    fn test_seed_dedups_toml_names_and_skips_empty() {
        let db = crate::db::Database::open_memory().unwrap();
        let mut dup = skill("a", &[], true);
        dup.content = "second".into();
        seed_skills(&db, &[skill("a", &[], true), dup, skill("  ", &[], true)]).unwrap();
        let s = stored(&db);
        assert_eq!(names(&s), vec!["a"]);
        assert_eq!(s[0].content, "do a things");
    }

    #[test]
    fn test_seed_leaves_corrupt_list_untouched() {
        let db = crate::db::Database::open_memory().unwrap();
        db.set_app_setting(crate::db::settings::SKILLS_KEY, "not json")
            .unwrap();
        seed_skills(&db, &[skill("a", &[], true)]).unwrap();
        assert_eq!(
            db.get_app_setting(crate::db::settings::SKILLS_KEY)
                .unwrap()
                .as_deref(),
            Some("not json")
        );
    }

    #[test]
    fn test_sanitize_skill_pipelines() {
        let mut s = skill("x", &["triage", "bogus"], true);
        let w = sanitize_skill_pipelines(&mut s);
        assert_eq!(s.pipelines, vec!["triage"]);
        assert!(s.enabled);
        assert_eq!(w.len(), 1);

        // Only unknown values: disabled, never widened to "all pipelines".
        let mut s = skill("y", &["diagram"], true);
        sanitize_skill_pipelines(&mut s);
        assert!(s.pipelines.is_empty() && !s.enabled);

        let mut s = skill("z", &[], true);
        assert!(sanitize_skill_pipelines(&mut s).is_empty());
        assert!(s.enabled);
    }

    #[test]
    fn test_seed_strips_unknown_pipelines() {
        let db = crate::db::Database::open_memory().unwrap();
        seed_skills(
            &db,
            &[
                skill("a", &["review", "nope"], true),
                skill("b", &["typo"], true),
            ],
        )
        .unwrap();
        let s = stored(&db);
        assert_eq!(s[0].pipelines, vec!["review"]);
        assert!(s[0].enabled);
        assert!(s[1].pipelines.is_empty() && !s[1].enabled);
        assert!(s.iter().all(|k| k
            .pipelines
            .iter()
            .all(|p| SKILL_PIPELINES.contains(&p.as_str()))));
    }

    /// Two "pods" seeding the same shared DB concurrently: the atomic
    /// insert-if-absent + CAS retry must converge on one list with every
    /// TOML skill exactly once.
    #[test]
    fn test_seed_concurrent_pods_converge() {
        let dir = std::env::temp_dir().join(format!(
            "wshm-seed-race-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("state.db");
        crate::db::Database::open_path(&path).unwrap();
        let toml_a = vec![skill("a", &[], true), skill("shared", &[], true)];
        let toml_b = vec![skill("b", &[], true), skill("shared", &[], true)];
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                let t = if i % 2 == 0 {
                    toml_a.clone()
                } else {
                    toml_b.clone()
                };
                std::thread::spawn(move || {
                    let db = crate::db::Database::open_path(&path).unwrap();
                    seed_skills(&db, &t).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let db = crate::db::Database::open_path(&path).unwrap();
        let mut n: Vec<String> = stored(&db).into_iter().map(|s| s.name).collect();
        n.sort();
        assert_eq!(n, vec!["a", "b", "shared"]);
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_sqlite_setting_primitives() {
        let db = crate::db::Database::open_memory().unwrap();
        assert!(db.set_app_setting_if_absent("k", "1").unwrap());
        assert!(!db.set_app_setting_if_absent("k", "2").unwrap());
        assert!(!db.compare_and_set_app_setting("k", "2", "3").unwrap());
        assert!(db.compare_and_set_app_setting("k", "1", "3").unwrap());
        assert_eq!(db.get_app_setting("k").unwrap().as_deref(), Some("3"));
        assert!(!db.compare_and_set_app_setting("missing", "x", "y").unwrap());
    }
}

#[cfg(test)]
mod rag_env_tests {
    use super::RagConfig;

    #[test]
    fn env_overrides_beat_toml_and_ignore_garbage() {
        let mut cfg: RagConfig = toml::from_str("enabled = true\ntop_k = 3\n").unwrap();
        assert_eq!(
            (cfg.enabled, cfg.top_k, cfg.max_terms, cfg.max_chars),
            (true, 3, 8, 3000)
        );
        std::env::set_var("WSHM_RAG_ENABLED", "off");
        std::env::set_var("WSHM_RAG_TOP_K", "9");
        std::env::set_var("WSHM_RAG_MAX_TERMS", "not-a-number");
        std::env::set_var("WSHM_RAG_MAX_CHARS", " 1200 ");
        cfg.apply_env_overrides();
        std::env::remove_var("WSHM_RAG_ENABLED");
        std::env::remove_var("WSHM_RAG_TOP_K");
        std::env::remove_var("WSHM_RAG_MAX_TERMS");
        std::env::remove_var("WSHM_RAG_MAX_CHARS");
        assert_eq!(
            (cfg.enabled, cfg.top_k, cfg.max_terms, cfg.max_chars),
            (false, 9, 8, 1200)
        );
    }
}

#[cfg(test)]
mod review_diagrams_tests {
    use super::*;

    #[test]
    fn defaults_are_on_short_english_mermaid_unpublished() {
        let c = Config::default();
        let d = &c.pr.review_diagrams;
        assert!(d.enabled);
        assert_eq!(d.kinds, vec!["architecture"]);
        assert_eq!(d.text, DiagramText::Short);
        assert_eq!(d.language, "en");
        assert_eq!(d.comment, DiagramComment::Mermaid);
        assert!(d.diff_viewer);
        assert_eq!(d.publish.target, DiagramPublishTarget::Off);
        assert!(!d.publish.allow_private);
    }

    #[test]
    fn toml_is_parsed() {
        let pr: PrConfig = toml::from_str(
            "[review_diagrams]\ntext = \"full\"\nlanguage = \"fr\"\n[review_diagrams.publish]\ntarget = \"github-pages\"\npages_repo = \"o/p\"\n",
        )
        .unwrap();
        assert_eq!(pr.review_diagrams.text, DiagramText::Full);
        assert_eq!(
            pr.review_diagrams.publish.target,
            DiagramPublishTarget::GithubPages
        );
        assert_eq!(
            pr.review_diagrams.publish.pages_repo.as_deref(),
            Some("o/p")
        );
        assert!(pr.review_diagrams.enabled, "unset keys keep their defaults");
    }

    #[test]
    fn db_override_is_merged_over_toml() {
        let mut toml = ReviewDiagramsConfig {
            language: "fr".into(),
            ..Default::default()
        };
        toml.publish.dir = Some("/srv".into());
        let over = serde_json::json!({"text": "normal", "publish": {"target": "local"}});
        let eff = apply_review_diagrams_override(&toml, &over).unwrap();
        assert_eq!(eff.text, DiagramText::Normal);
        assert_eq!(eff.language, "fr", "TOML value kept");
        assert_eq!(eff.publish.target, DiagramPublishTarget::Local);
        assert_eq!(
            eff.publish.dir.as_deref(),
            Some("/srv"),
            "nested TOML value kept"
        );
        assert!(
            apply_review_diagrams_override(&toml, &serde_json::json!({"text": "huge"})).is_err()
        );
    }

    #[test]
    fn toml_only_keys_are_detected_and_ignored() {
        let over = serde_json::json!({"publish": {"dir": "/etc", "allow_private": true, "target": "local"}});
        assert_eq!(
            toml_only_keys(&over),
            vec!["publish.dir", "publish.allow_private"]
        );
        let mut toml = ReviewDiagramsConfig::default();
        toml.publish.dir = Some("/srv".into());
        let eff = apply_review_diagrams_override(&toml, &over).unwrap();
        assert_eq!(eff.publish.dir.as_deref(), Some("/srv"));
        assert!(!eff.publish.allow_private);
        assert_eq!(eff.publish.target, DiagramPublishTarget::Local);
        assert!(toml_only_keys(&serde_json::json!({"text": "full"})).is_empty());
    }

    #[test]
    fn unknown_override_keys_are_reported() {
        let reference = serde_json::to_value(ReviewDiagramsConfig::default()).unwrap();
        let over = serde_json::json!({"txt": "x", "publish": {"target": "off", "secret_key": "k"}});
        let mut keys = unknown_json_keys(&reference, &over);
        keys.sort();
        assert_eq!(keys, vec!["publish.secret_key", "txt"]);
    }
}
