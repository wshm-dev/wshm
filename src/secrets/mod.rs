//! Encrypted secret store — application-managed secrets stored encrypted in
//! a SQLite database, decrypted in-memory when read.
//!
//! Distinct from the `crate::vault` module which integrates with EXTERNAL
//! vault providers (HashiCorp Vault, AWS Secrets Manager, …). This module
//! provides an INTERNAL store the user manages through the web UI: each
//! row is encrypted with AES-256-GCM under a master key loaded from the
//! `WSHM_MASTER_KEY` env var (hex-encoded 32 bytes).
//!
//! AAD (additional authenticated data) on every record binds the ciphertext
//! to its scope+slug+key triple, so an attacker who exfiltrates one row
//! cannot paste its ciphertext under another key without the AEAD tag
//! failing.

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

mod cipher;
mod store;

pub use cipher::{aad_for, aad_for_legacy, Cipher, MasterKey};
pub use store::{SecretRecord, SqliteSecretStore};

/// Backend-agnostic interface for the encrypted secret store.
///
/// OSS ships [`SqliteSecretStore`]; downstream binaries (e.g. wshm-pro)
/// can plug in their own backend (Postgres, etc.) by implementing this
/// trait and passing the boxed value through `DaemonExtensions.secrets`.
///
/// The cipher (AES-256-GCM) wraps every plaintext before it leaves the
/// process; persistence layers see only ciphertext + nonce + AAD.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// List all stored secrets, values masked.
    async fn list(&self) -> Result<Vec<SecretRecord>>;

    /// Insert or update a secret. Returns the row id.
    async fn put(
        &self,
        scope: Scope,
        slug: Option<&str>,
        key: &str,
        plaintext: &str,
        updated_by: Option<i64>,
    ) -> Result<i64>;

    /// Read & decrypt a secret by logical identity (scope+slug+key).
    /// Returns `None` if not present.
    async fn get(&self, scope: Scope, slug: Option<&str>, key: &str) -> Result<Option<String>>;

    /// Synchronous variant of [`Self::get`] usable from non-async daemon
    /// code (scheduler/poller). Implementations may share a connection
    /// pool but must NOT block the async runtime — use a separate sync
    /// connection or `block_in_place`.
    fn get_blocking(&self, scope: Scope, slug: Option<&str>, key: &str) -> Result<Option<String>>;

    /// Decrypt a secret by row id and write an audit row.
    async fn reveal(&self, id: i64, user_id: Option<i64>) -> Result<Option<String>>;

    /// Delete a secret by row id, with audit. Returns true if a row was
    /// deleted.
    async fn delete(&self, id: i64, user_id: Option<i64>) -> Result<bool>;
}

/// Where a secret applies — `Global` for daemon-wide values, `Repo` for
/// per-repository overrides keyed by `owner/repo` slug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Repo,
}

impl Scope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Repo => "repo",
        }
    }
    #[allow(clippy::should_implement_trait)] // returns anyhow::Result, not std FromStr's Err shape
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "global" => Ok(Scope::Global),
            "repo" => Ok(Scope::Repo),
            other => Err(anyhow!("invalid scope: {other}")),
        }
    }
}

/// Resolve a secret value: look in `store` first (most specific scope wins),
/// then fall back to the named env var. Returns `None` if neither is found.
///
/// Lookup order for a per-repo lookup:
///   1. `secrets[scope=repo, slug=<slug>, key=<key>]`
///   2. `secrets[scope=global, key=<key>]`
///   3. `std::env::var(<env_name>)`
pub fn resolve(
    store: Option<&Arc<dyn SecretStore>>,
    repo_slug: Option<&str>,
    key: &str,
    env_name: &str,
) -> Option<String> {
    if let Some(s) = store {
        if let Some(slug) = repo_slug {
            if let Ok(Some(v)) = s.get_blocking(Scope::Repo, Some(slug), key) {
                return Some(v);
            }
        }
        if let Ok(Some(v)) = s.get_blocking(Scope::Global, None, key) {
            return Some(v);
        }
    }
    std::env::var(env_name).ok().filter(|v| !v.is_empty())
}

/// Process-wide secret store handle, populated by the daemon at startup so
/// non-async callers (e.g. `Config::github_token`) can resolve secrets
/// without threading the store through every signature.
static GLOBAL: OnceLock<Arc<dyn SecretStore>> = OnceLock::new();

/// Install the process-wide secret store. Idempotent — subsequent calls are
/// no-ops so a `cargo test` calling `install_global` twice doesn't panic.
pub fn install_global(store: Arc<dyn SecretStore>) {
    let _ = GLOBAL.set(store);
}

/// Returns the process-wide secret store if [`install_global`] was called.
pub fn global() -> Option<Arc<dyn SecretStore>> {
    GLOBAL.get().cloned()
}

/// Sanity check on the master key: must be exactly 32 bytes of hex.
pub fn validate_master_key(hex_str: &str) -> Result<()> {
    let bytes = hex::decode(hex_str.trim()).context("WSHM_MASTER_KEY is not valid hex")?;
    if bytes.len() != 32 {
        bail!(
            "WSHM_MASTER_KEY must be 32 bytes (64 hex chars), got {} bytes",
            bytes.len()
        );
    }
    Ok(())
}
