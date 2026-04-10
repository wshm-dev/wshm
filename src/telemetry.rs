//! Anonymous telemetry with explicit GDPR opt-in consent.
//!
//! Telemetry is OFF by default. On first interactive run, wshm prompts the
//! user to accept or decline. The decision is stored in
//! `~/.wshm/telemetry-consent` and never asked again.
//!
//! What we collect (ONLY if user accepts):
//! - Anonymized device hash (SHA256 of salt + hostname + username)
//! - wshm version
//! - OS and architecture
//! - Number of configured repos (count only, no names)
//! - Install method (homebrew / cargo / manual)
//!
//! What we NEVER collect:
//! - Repository names, URLs, or content
//! - Issue or PR content
//! - API tokens or credentials
//! - Code or configuration values
//!
//! Opt-out / withdraw consent:
//! - `wshm telemetry --decline` to disable
//! - `wshm telemetry --status` to check current state
//! - Set `WSHM_TELEMETRY_DISABLED=1` env var
//! - Delete `~/.wshm/telemetry-consent`

use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

const TELEMETRY_URL: Option<&str> = option_env!("WSHM_TELEMETRY_URL");
// Default build: https://telemetry.wshm.dev/api/v1/telemetry
const PING_INTERVAL_SECS: u64 = 23 * 3600; // 23 hours

static CACHED_SALT: OnceLock<String> = OnceLock::new();

/// Consent state stored in ~/.wshm/telemetry-consent
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConsentState {
    /// User has not yet been asked.
    Unknown,
    /// User explicitly accepted.
    Accepted,
    /// User explicitly declined.
    Declined,
}

/// Read the user's telemetry consent state.
pub fn consent_state() -> ConsentState {
    let path = consent_file_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return ConsentState::Unknown;
    };
    match content.trim() {
        "accepted" => ConsentState::Accepted,
        "declined" => ConsentState::Declined,
        _ => ConsentState::Unknown,
    }
}

/// Record the user's consent decision.
pub fn set_consent(accepted: bool) -> std::io::Result<()> {
    let path = consent_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let value = if accepted { "accepted" } else { "declined" };
    std::fs::write(&path, value)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Prompt the user for telemetry consent on first run.
///
/// This is interactive — it reads from stdin. In non-interactive contexts
/// (CI, daemon, non-TTY stdin) we default to Declined to respect GDPR.
pub fn prompt_consent_if_needed() -> ConsentState {
    // Already decided
    let current = consent_state();
    if current != ConsentState::Unknown {
        return current;
    }

    // Non-interactive detection: CI, non-TTY, or env override
    if std::env::var("CI").is_ok() || std::env::var("WSHM_TELEMETRY_DISABLED").ok().as_deref() == Some("1") {
        let _ = set_consent(false);
        return ConsentState::Declined;
    }

    if !is_stdin_tty() {
        // Non-interactive: don't bother the user, default to declined.
        // They can opt in later with `wshm telemetry --accept`.
        return ConsentState::Unknown;
    }

    // Interactive prompt
    println!();
    println!("──────────────────────────────────────────────────────────");
    println!("  Help improve wshm");
    println!("──────────────────────────────────────────────────────────");
    println!();
    println!("wshm can send anonymous usage data to help us understand");
    println!("how the tool is used and improve it.");
    println!();
    println!("What we collect (ONLY if you accept):");
    println!("  • Anonymous device hash (SHA256, not reversible)");
    println!("  • wshm version, OS, architecture");
    println!("  • Number of configured repos (count only)");
    println!("  • Install method (brew / cargo / manual)");
    println!();
    println!("What we NEVER collect:");
    println!("  • Repository names, URLs, or content");
    println!("  • Issue or PR content");
    println!("  • API tokens or credentials");
    println!("  • Any code or configuration");
    println!();
    println!("You can change your choice anytime with:");
    println!("  wshm telemetry --accept");
    println!("  wshm telemetry --decline");
    println!();
    print!("Accept anonymous telemetry? [y/N]: ");
    let _ = std::io::stdout().flush();

    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return ConsentState::Unknown;
    }
    let accepted = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
    let _ = set_consent(accepted);

    if accepted {
        println!("\nThank you! You can withdraw consent anytime with `wshm telemetry --decline`.\n");
        ConsentState::Accepted
    } else {
        println!("\nNo telemetry will be sent. You can enable it later with `wshm telemetry --accept`.\n");
        ConsentState::Declined
    }
}

fn is_stdin_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

fn consent_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".wshm")
        .join("telemetry-consent")
}

/// Send a telemetry ping if enabled and not already sent today.
/// Fire-and-forget: errors are silently ignored.
///
/// Telemetry is only sent if:
/// - The binary was built with WSHM_TELEMETRY_URL
/// - The user has explicitly accepted consent
/// - WSHM_TELEMETRY_DISABLED is NOT set to "1"
/// - The last ping was > 23 hours ago
pub fn maybe_ping() {
    if TELEMETRY_URL.is_none() {
        return;
    }

    // Opt-out env var always wins
    if std::env::var("WSHM_TELEMETRY_DISABLED").unwrap_or_default() == "1" {
        return;
    }

    // GDPR: require explicit consent
    if consent_state() != ConsentState::Accepted {
        return;
    }

    // Rate limit
    let marker = telemetry_marker_path();
    if let Ok(metadata) = std::fs::metadata(&marker) {
        if let Ok(modified) = metadata.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed.as_secs() < PING_INTERVAL_SECS {
                    return;
                }
            }
        }
    }

    touch_marker(&marker);

    std::thread::spawn(|| {
        let _ = send_ping();
    });
}

fn send_ping() -> Result<(), Box<dyn std::error::Error>> {
    let url = TELEMETRY_URL.ok_or("no telemetry URL")?;
    let device_hash = generate_device_hash();
    let version = env!("CARGO_PKG_VERSION").to_string();
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();

    // Get basic stats
    let repos = count_configured_repos();

    let payload = serde_json::json!({
        "machine_id": device_hash,
        "event": "ping",
        "data": {
            "version": version,
            "os": os,
            "arch": arch,
            "repos": repos,
            "install_method": detect_install_method(),
        },
        "app_version": version,
    });

    ureq::post(url)
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(2))
        .send_string(&payload.to_string())?;

    Ok(())
}

fn generate_device_hash() -> String {
    let salt = get_or_create_salt();
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b":");
    hasher.update(hostname.as_bytes());
    hasher.update(b":");
    hasher.update(username.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn get_or_create_salt() -> String {
    CACHED_SALT
        .get_or_init(|| {
            let salt_path = salt_file_path();

            // Try to read existing salt
            if let Ok(contents) = std::fs::read_to_string(&salt_path) {
                let trimmed = contents.trim().to_string();
                if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                    return trimmed;
                }
            }

            // Generate new random salt
            let salt = random_salt();
            if let Some(parent) = salt_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::File::create(&salt_path) {
                let _ = f.write_all(salt.as_bytes());
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &salt_path,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
            }
            salt
        })
        .clone()
}

fn random_salt() -> String {
    let mut buf = [0u8; 32];
    if getrandom::getrandom(&mut buf).is_err() {
        // Fallback: use time + pid as entropy source
        let fallback = format!("{:?}:{}", std::time::SystemTime::now(), std::process::id());
        let mut hasher = Sha256::new();
        hasher.update(fallback.as_bytes());
        return format!("{:x}", hasher.finalize());
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

fn salt_file_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("wshm")
        .join(".device_salt")
}

fn count_configured_repos() -> usize {
    let config_path = dirs::home_dir()
        .map(|h| h.join(".wshm").join("config.toml"))
        .unwrap_or_default();
    let content = std::fs::read_to_string(&config_path).unwrap_or_default();
    content.matches("[[repos]]").count()
}

fn detect_install_method() -> &'static str {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return "unknown",
    };
    let path = std::fs::canonicalize(&exe)
        .unwrap_or(exe)
        .to_string_lossy()
        .to_string();

    if path.contains("homebrew") || path.contains("Cellar") { "homebrew" }
    else if path.contains("cargo") { "cargo" }
    else if path.contains(".local/bin") { "manual" }
    else { "unknown" }
}

fn telemetry_marker_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("wshm-telemetry-ping")
}

fn touch_marker(path: &PathBuf) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, "");
}
