//! Pro hooks registry — injected at startup by wshm-pro.
//! In the OSS build, all hooks are None (no-op).
//!
//! Every hook is stored in a `OnceLock` (process-global). Earlier versions
//! used `thread_local!`, which silently broke under tokio's multi-threaded
//! runtime: a hook set on the main thread was invisible from worker
//! threads, so `is_pro()` / `has_feature()` returned `false` non-
//! deterministically on a Pro build, and `apply_output_hook` skipped
//! redaction on most events.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use crate::config::Config;
use crate::db::backend::DatabaseBackend;
use crate::git_provider::Forge as GhClient;

// --- Feature gate hook ---
// Checks if a pro feature is available (license validation).
type FeatureGateFn = fn(&str) -> bool;

static FEATURE_GATE: OnceLock<FeatureGateFn> = OnceLock::new();

pub fn set_feature_gate(f: FeatureGateFn) {
    let _ = FEATURE_GATE.set(f);
}

/// Check if a pro feature is available.
/// Returns false in OSS build (no hook registered).
pub fn has_feature(feature: &str) -> bool {
    FEATURE_GATE.get().map(|f| f(feature)).unwrap_or(false)
}

/// Returns true if running as pro edition.
pub fn is_pro() -> bool {
    FEATURE_GATE.get().is_some()
}

// --- Pro version override ---
// Pro registers its own CARGO_PKG_VERSION here (e.g. "0.31.1-pro") so
// `/api/v1/license` can advertise it instead of the wshm-core path-dep
// version. OSS leaves this unset and falls back to env!("CARGO_PKG_VERSION").

static PRO_VERSION: OnceLock<&'static str> = OnceLock::new();

pub fn set_pro_version(v: &'static str) {
    let _ = PRO_VERSION.set(v);
}

pub fn pro_version() -> Option<&'static str> {
    PRO_VERSION.get().copied()
}

// --- Cloud sync hook ---
// Called after telemetry/events to sync with cloud.
type SyncHookFn = fn(&str, &serde_json::Value);

static SYNC_HOOK: OnceLock<SyncHookFn> = OnceLock::new();

pub fn set_sync_hook(f: SyncHookFn) {
    let _ = SYNC_HOOK.set(f);
}

pub fn maybe_sync(event: &str, data: &serde_json::Value) {
    if let Some(f) = SYNC_HOOK.get() {
        f(event, data);
    }
}

// --- Output filter hook ---
// Intercepts output for redaction (e.g., secret shielding).
type OutputHookFn = fn(&str) -> String;

static OUTPUT_HOOK: OnceLock<OutputHookFn> = OnceLock::new();

pub fn set_output_hook(f: OutputHookFn) {
    let _ = OUTPUT_HOOK.set(f);
}

pub fn apply_output_hook(text: &str) -> String {
    match OUTPUT_HOOK.get() {
        Some(f) => f(text),
        None => text.to_string(),
    }
}

// --- Review / auto-fix entry points ---
// Inline review and auto-fix are part of wshm-core. These thin wrappers are
// kept so existing call sites (triage auto-fix, daemon slash commands, the
// PR event processor) keep a stable entry point. They always run the
// pipeline and return `Ok(true)`.

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Run the auto-fix pipeline for a given issue number (opens a fix PR).
pub async fn run_auto_fix(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    issue_number: u64,
) -> anyhow::Result<bool> {
    let fix_args = crate::cli::FixArgs {
        issue: issue_number,
        tool: None,
        model: None,
        docker: false,
        image: None,
        apply: true,
    };
    crate::pipelines::autogen::run(config, db, gh, &fix_args, None).await?;
    Ok(true)
}

/// Run the inline review pipeline for a given PR number.
pub async fn run_review(
    config: &Config,
    db: &dyn DatabaseBackend,
    gh: &GhClient,
    pr_number: u64,
    apply: bool,
) -> anyhow::Result<bool> {
    let args = crate::cli::ReviewArgs {
        pr: Some(pr_number),
        apply,
    };
    crate::pipelines::review::run(config, db, gh, &args, false).await?;
    Ok(true)
}

// --- Update hooks ---
// Allow wshm-pro to override the update target (binary name, repo, version
// suffix) without duplicating the download/verify/replace logic.

pub type UpdateFn = fn(bool, bool) -> BoxFuture<'static, anyhow::Result<Option<String>>>;
pub type AutoUpdateFn = fn() -> BoxFuture<'static, ()>;

static UPDATE_HOOK: OnceLock<UpdateFn> = OnceLock::new();
static AUTO_UPDATE_HOOK: OnceLock<AutoUpdateFn> = OnceLock::new();

pub fn set_update_hook(f: UpdateFn) {
    let _ = UPDATE_HOOK.set(f);
}

pub fn set_auto_update_hook(f: AutoUpdateFn) {
    let _ = AUTO_UPDATE_HOOK.set(f);
}

/// Check/apply an update. Falls back to the OSS config if no hook is registered.
pub async fn run_update(apply: bool, json: bool) -> anyhow::Result<Option<String>> {
    match UPDATE_HOOK.get() {
        Some(f) => f(apply, json).await,
        None => {
            crate::update::check_and_update(&crate::update::UpdateConfig::oss(), apply, json).await
        }
    }
}

/// Silent background update. Falls back to the OSS config if no hook is registered.
pub async fn run_auto_update() {
    match AUTO_UPDATE_HOOK.get() {
        Some(f) => f().await,
        None => crate::update::auto_check_and_update(&crate::update::UpdateConfig::oss()).await,
    }
}

// --- Web extensions hook ---
// NOTE: In OSS build, daemon and web functionality are not available.
// This hook is preserved for Pro builds which will provide the daemon types.
// In OSS, these types and functions are not used.
