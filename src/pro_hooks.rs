//! Pro hooks registry — injected at startup by wshm-pro.
//! In the OSS build, all hooks are None (no-op).

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use crate::config::Config;
use crate::db::Database;
use crate::github::Client as GhClient;

// --- Feature gate hook ---
// Checks if a pro feature is available (license validation).
type FeatureGateFn = fn(&str) -> bool;

thread_local! {
    static FEATURE_GATE: RefCell<Option<FeatureGateFn>> = const { RefCell::new(None) };
}

pub fn set_feature_gate(f: FeatureGateFn) {
    FEATURE_GATE.with(|cell| {
        *cell.borrow_mut() = Some(f);
    });
}

/// Check if a pro feature is available.
/// Returns false in OSS build (no hook registered).
pub fn has_feature(feature: &str) -> bool {
    FEATURE_GATE.with(|cell| cell.borrow().map(|f| f(feature)).unwrap_or(false))
}

/// Returns true if running as pro edition.
pub fn is_pro() -> bool {
    FEATURE_GATE.with(|cell| cell.borrow().is_some())
}

// --- Cloud sync hook ---
// Called after telemetry/events to sync with cloud.
type SyncHookFn = fn(&str, &serde_json::Value);

thread_local! {
    static SYNC_HOOK: RefCell<Option<SyncHookFn>> = const { RefCell::new(None) };
}

pub fn set_sync_hook(f: SyncHookFn) {
    SYNC_HOOK.with(|cell| {
        *cell.borrow_mut() = Some(f);
    });
}

pub fn maybe_sync(event: &str, data: &serde_json::Value) {
    SYNC_HOOK.with(|cell| {
        if let Some(f) = *cell.borrow() {
            f(event, data);
        }
    });
}

// --- Output filter hook ---
// Intercepts output for redaction (e.g., secret shielding).
type OutputHookFn = fn(&str) -> String;

thread_local! {
    static OUTPUT_HOOK: RefCell<Option<OutputHookFn>> = const { RefCell::new(None) };
}

pub fn set_output_hook(f: OutputHookFn) {
    OUTPUT_HOOK.with(|cell| {
        *cell.borrow_mut() = Some(f);
    });
}

pub fn apply_output_hook(text: &str) -> String {
    OUTPUT_HOOK.with(|cell| match *cell.borrow() {
        Some(f) => f(text),
        None => text.to_string(),
    })
}

// --- Async Pro pipeline hooks ---
// These allow wshm-core code paths (triage auto-fix, daemon slash commands)
// to invoke Pro-only pipelines (autogen, review) without depending on Pro
// source code at compile time.
//
// Each hook takes a borrowed context (Config, Database, Client, plus a
// minimal set of primitive args) and returns a boxed future resolving to
// `anyhow::Result<()>`. In the OSS build the hook is `None` and calls are
// skipped with a warning.
//
// Note: we use `OnceLock` + `&'static dyn` rather than `thread_local!` to
// ensure the hook survives tokio task migrations across worker threads.

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type AutoFixHook =
    for<'a> fn(&'a Config, &'a Database, &'a GhClient, u64) -> BoxFuture<'a, anyhow::Result<()>>;

pub type ReviewHook = for<'a> fn(
    &'a Config,
    &'a Database,
    &'a GhClient,
    u64,
    bool,
) -> BoxFuture<'a, anyhow::Result<()>>;

static AUTO_FIX_HOOK: OnceLock<AutoFixHook> = OnceLock::new();
static REVIEW_HOOK: OnceLock<ReviewHook> = OnceLock::new();

pub fn set_auto_fix_hook(f: AutoFixHook) {
    let _ = AUTO_FIX_HOOK.set(f);
}

pub fn set_review_hook(f: ReviewHook) {
    let _ = REVIEW_HOOK.set(f);
}

/// Invoke the Pro auto-fix pipeline for a given issue number.
/// Returns `Ok(false)` if no Pro hook is registered (OSS build).
pub async fn run_auto_fix(
    config: &Config,
    db: &Database,
    gh: &GhClient,
    issue_number: u64,
) -> anyhow::Result<bool> {
    match AUTO_FIX_HOOK.get() {
        Some(f) => {
            f(config, db, gh, issue_number).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Invoke the Pro review pipeline for a given PR number.
/// Returns `Ok(false)` if no Pro hook is registered (OSS build).
pub async fn run_review(
    config: &Config,
    db: &Database,
    gh: &GhClient,
    pr_number: u64,
    apply: bool,
) -> anyhow::Result<bool> {
    match REVIEW_HOOK.get() {
        Some(f) => {
            f(config, db, gh, pr_number, apply).await?;
            Ok(true)
        }
        None => Ok(false),
    }
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
