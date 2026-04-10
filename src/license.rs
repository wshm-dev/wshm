use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

use crate::config::LicenseConfig;

const LICENSE_API: &str = "https://api.wshm.dev/api/v1/license";

fn credentials_path() -> PathBuf {
    PathBuf::from(".wshm").join("credentials")
}

fn default_token_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".wshm")
        .join("license.jwt")
}

// ---------------------------------------------------------------------------
// License resolution chain
// ---------------------------------------------------------------------------

/// Resolved license: either a raw key (needs activation) or a cached JWT.
pub enum ResolvedLicense {
    /// A license key that can be activated (e.g. WSHM-XXXX-XXXX-XXXX)
    Key(String),
    /// A cached JWT token (already activated)
    Jwt(String),
    /// No license found
    None,
}

/// Resolve the license using the full resolution chain:
///
/// 1. Vault placeholder in config `[license] key = "vault(...)"`
/// 2. Environment variable `WSHM_LICENSE_KEY`
/// 3. Config `[license] path = "..."` (JWT file)
/// 4. Fallback `~/.wshm/license.jwt`
pub async fn resolve(config: &LicenseConfig, vault_config: Option<&crate::config::VaultConfig>) -> ResolvedLicense {
    // Step 1: Check config key field (may contain vault placeholder)
    if let Some(ref key_value) = config.key {
        let key_value = key_value.trim();
        if !key_value.is_empty() {
            if crate::vault::has_vault_placeholders(key_value) {
                // Resolve vault placeholder
                match resolve_from_vault(key_value, vault_config).await {
                    Some(resolved) => {
                        tracing::debug!("License resolved from vault");
                        return if looks_like_jwt(&resolved) {
                            ResolvedLicense::Jwt(resolved)
                        } else {
                            ResolvedLicense::Key(resolved)
                        };
                    }
                    None => {
                        tracing::warn!("Vault placeholder in [license] key could not be resolved");
                    }
                }
            } else {
                // Plain value in config (shouldn't be used but handle gracefully)
                tracing::warn!("License key in config.toml is not protected by vault — consider using vault() or WSHM_LICENSE_KEY env var");
                return if looks_like_jwt(key_value) {
                    ResolvedLicense::Jwt(key_value.to_string())
                } else {
                    ResolvedLicense::Key(key_value.to_string())
                };
            }
        }
    }

    // Step 2: Environment variable
    if let Ok(env_key) = std::env::var("WSHM_LICENSE_KEY") {
        let env_key = env_key.trim().to_string();
        if !env_key.is_empty() {
            tracing::debug!("License resolved from WSHM_LICENSE_KEY env var");
            return if looks_like_jwt(&env_key) {
                ResolvedLicense::Jwt(env_key)
            } else {
                ResolvedLicense::Key(env_key)
            };
        }
    }

    // Step 3: Config path field (explicit JWT file path)
    if let Some(ref path_str) = config.path {
        let path = expand_tilde(path_str);
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                let content = content.trim().to_string();
                if !content.is_empty() {
                    tracing::debug!("License JWT loaded from config path: {}", path.display());
                    return ResolvedLicense::Jwt(content);
                }
            }
        } else {
            tracing::debug!("License path {} does not exist", path.display());
        }
    }

    // Step 4: Fallback to default path ~/.wshm/license.jwt
    let default_path = default_token_path();
    if default_path.exists() {
        if let Ok(content) = fs::read_to_string(&default_path) {
            let content = content.trim().to_string();
            if !content.is_empty() {
                tracing::debug!("License JWT loaded from {}", default_path.display());
                return ResolvedLicense::Jwt(content);
            }
        }
    }

    ResolvedLicense::None
}

/// Synchronous version for contexts where async isn't available.
pub fn resolve_sync(config: &LicenseConfig) -> ResolvedLicense {
    // Step 1: Config key (vault not available in sync context — skip)
    if let Some(ref key_value) = config.key {
        let key_value = key_value.trim();
        if !key_value.is_empty() && !crate::vault::has_vault_placeholders(key_value) {
            return if looks_like_jwt(key_value) {
                ResolvedLicense::Jwt(key_value.to_string())
            } else {
                ResolvedLicense::Key(key_value.to_string())
            };
        }
    }

    // Step 2: Environment variable
    if let Ok(env_key) = std::env::var("WSHM_LICENSE_KEY") {
        let env_key = env_key.trim().to_string();
        if !env_key.is_empty() {
            return if looks_like_jwt(&env_key) {
                ResolvedLicense::Jwt(env_key)
            } else {
                ResolvedLicense::Key(env_key)
            };
        }
    }

    // Step 3: Config path
    if let Some(ref path_str) = config.path {
        let path = expand_tilde(path_str);
        if let Ok(content) = fs::read_to_string(&path) {
            let content = content.trim().to_string();
            if !content.is_empty() {
                return ResolvedLicense::Jwt(content);
            }
        }
    }

    // Step 4: Fallback
    let default_path = default_token_path();
    if let Ok(content) = fs::read_to_string(&default_path) {
        let content = content.trim().to_string();
        if !content.is_empty() {
            return ResolvedLicense::Jwt(content);
        }
    }

    ResolvedLicense::None
}

async fn resolve_from_vault(placeholder: &str, vault_config: Option<&crate::config::VaultConfig>) -> Option<String> {
    let vc = vault_config?;
    let resolver = crate::vault::build_resolver(vc).ok()??;
    crate::vault::resolve_placeholders(placeholder, resolver.as_ref())
        .await
        .ok()
}

fn looks_like_jwt(s: &str) -> bool {
    // JWTs have 3 base64 segments separated by dots
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| p.len() > 10)
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

// ---------------------------------------------------------------------------
// Interactive login
// ---------------------------------------------------------------------------

/// Interactive login flow: ask for license key, activate, cache JWT.
pub fn login() -> Result<()> {
    println!("\n── License ──");
    println!("Enter your license key (get one at https://wshm.dev):\n");

    print!("License key: ");
    std::io::Write::flush(&mut std::io::stdout())?;
    let mut key = String::new();
    std::io::stdin().read_line(&mut key)?;
    let key = key.trim();

    if key.is_empty() {
        println!("Skipped.");
        return Ok(());
    }

    activate_key(key)
}

/// Activate a license key: call API, cache JWT.
pub fn activate_key(key: &str) -> Result<()> {
    // Save to credentials
    save_credential("LICENSE_KEY", key)?;

    let machine_id = generate_machine_id();

    let resp = ureq::post(&format!("{LICENSE_API}/activate"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(10))
        .send_string(
            &serde_json::json!({
                "license_key": key,
                "machine_id": machine_id,
                "app_version": env!("CARGO_PKG_VERSION"),
            })
            .to_string(),
        );

    match resp {
        Ok(r) => {
            let body: serde_json::Value = r.into_json().unwrap_or_default();
            if let Some(token) = body["token"].as_str() {
                cache_token(token)?;
                let plan = body["license"]["type"].as_str().unwrap_or("pro");
                println!("License activated — plan: {plan}");
                Ok(())
            } else {
                anyhow::bail!("Unexpected response: {body}");
            }
        }
        Err(ureq::Error::Status(code, _)) => {
            anyhow::bail!("Validation failed (HTTP {code})");
        }
        Err(e) => {
            anyhow::bail!("Cannot reach license server: {e}");
        }
    }
}

/// Activate a resolved license (auto-activate keys, return JWT directly).
pub async fn activate_resolved(
    resolved: ResolvedLicense,
    vault_config: Option<&crate::config::VaultConfig>,
) -> Option<String> {
    match resolved {
        ResolvedLicense::Jwt(jwt) => Some(jwt),
        ResolvedLicense::Key(key) => {
            // Try to activate the key and get a JWT
            tracing::info!("Activating license key...");
            match activate_key(&key) {
                Ok(()) => {
                    // Read the cached JWT
                    fs::read_to_string(default_token_path()).ok()
                }
                Err(e) => {
                    tracing::warn!("License activation failed: {e}");
                    None
                }
            }
        }
        ResolvedLicense::None => {
            let _ = vault_config; // suppress unused warning
            None
        }
    }
}

fn cache_token(token: &str) -> Result<()> {
    let path = default_token_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn generate_machine_id() -> String {
    use sha2::{Digest, Sha256};
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(format!("{hostname}:{username}"));
    format!("{:x}", hasher.finalize())
}

fn save_credential(key: &str, value: &str) -> Result<()> {
    let path = credentials_path();
    let mut creds: Vec<(String, String)> = Vec::new();

    if let Ok(content) = fs::read_to_string(&path) {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() != key {
                    creds.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
        }
    }
    creds.push((key.to_string(), value.to_string()));

    let content: String = creds
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, format!("# wshm credentials\n{content}\n"))
        .context("Failed to save credentials")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
