use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tracing::{info, warn};

/// Repo for release downloads. Hardcoded for supply-chain safety.
/// Points to homebrew-tap which hosts wshm-pro binaries.
const REPO: &str = "wshm-dev/homebrew-tap";

/// Current version from Cargo.toml
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Detect the right asset name for this platform.
fn asset_target() -> Result<&'static str> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match (os, arch) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        _ => anyhow::bail!("Unsupported platform: {os}/{arch}"),
    }
}

/// Get the repo to check for updates. Hardcoded — no override allowed.
fn update_repo() -> String {
    REPO.to_string()
}

/// Fetch latest release info from GitHub API.
async fn fetch_latest_release(
    http: &reqwest::Client,
    token: Option<&str>,
) -> Result<(String, String)> {
    let repo = update_repo();
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");

    let mut req = http
        .get(&url)
        .header("User-Agent", "wshm-updater")
        .header("Accept", "application/vnd.github+json");

    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }

    let resp = req.send().await.context("Failed to fetch latest release")?;
    let status = resp.status();
    let body = resp.text().await?;

    if !status.is_success() {
        let end = crate::ai::prompts::truncate_utf8(&body, 200);
        let safe_body = &body[..end];
        anyhow::bail!("GitHub API error ({status}): {safe_body}");
    }

    let json: serde_json::Value = serde_json::from_str(&body)?;
    let tag = json["tag_name"]
        .as_str()
        .context("Missing tag_name in release")?
        .to_string();
    let html_url = json["html_url"].as_str().unwrap_or("").to_string();

    Ok((tag, html_url))
}

/// Fetch release assets list from the GitHub API (works for private repos).
async fn fetch_release_assets(
    http: &reqwest::Client,
    tag: &str,
    token: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let repo = update_repo();
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");

    let mut req = http
        .get(&url)
        .header("User-Agent", "wshm-updater")
        .header("Accept", "application/vnd.github+json");
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }

    let resp = req.send().await.context("Failed to fetch release assets")?;
    if !resp.status().is_success() {
        anyhow::bail!("Failed to fetch release {tag} ({})", resp.status());
    }

    let json: serde_json::Value = serde_json::from_str(&resp.text().await?)?;
    let assets = json["assets"]
        .as_array()
        .context("Missing assets in release")?;

    let mut result = Vec::new();
    for asset in assets {
        let name = asset["name"].as_str().unwrap_or("").to_string();
        let download_url = asset["url"].as_str().unwrap_or("").to_string();
        if !name.is_empty() && !download_url.is_empty() {
            result.push((name, download_url));
        }
    }

    Ok(result)
}

/// Download a release asset by its API URL (works for private repos).
async fn download_asset(
    http: &reqwest::Client,
    api_url: &str,
    token: Option<&str>,
) -> Result<Vec<u8>> {
    let mut req = http
        .get(api_url)
        .header("User-Agent", "wshm-updater")
        .header("Accept", "application/octet-stream");
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }

    let resp = req.send().await.context("Failed to download asset")?;
    if !resp.status().is_success() {
        anyhow::bail!("Failed to download asset ({})", resp.status());
    }

    Ok(resp.bytes().await?.to_vec())
}

/// Download checksums file from a release (via API for private repo support).
async fn fetch_checksums(http: &reqwest::Client, tag: &str, token: Option<&str>, assets: &[(String, String)]) -> Result<String> {
    // Look for checksums file in assets
    let checksums_name = format!("checksums-{tag}.sha256");
    let asset_url = assets
        .iter()
        .find(|(name, _)| name == &checksums_name || name == "checksums.txt")
        .map(|(_, url)| url.as_str())
        .with_context(|| format!("No checksums file found in release {tag}"))?;

    let data = download_asset(http, asset_url, token).await?;
    String::from_utf8(data).context("Checksums file is not valid UTF-8")
}

/// Parse expected SHA256 for a given target from checksums.txt.
fn parse_checksum(checksums: &str, target: &str) -> Result<String> {
    for line in checksums.lines() {
        if line.contains(target) {
            let hash = line
                .split_whitespace()
                .next()
                .context("Invalid checksum line")?;
            return Ok(hash.to_string());
        }
    }
    anyhow::bail!("No checksum found for target {target} in checksums.txt")
}

/// Download the release binary for this platform (via API for private repo support).
async fn download_binary(
    http: &reqwest::Client,
    target: &str,
    token: Option<&str>,
    assets: &[(String, String)],
) -> Result<Vec<u8>> {
    let ext = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    let asset_name = format!("wshm-{target}.{ext}");

    let asset_url = assets
        .iter()
        .find(|(name, _)| name == &asset_name)
        .map(|(_, url)| url.as_str())
        .with_context(|| format!("Asset {asset_name} not found in release"))?;

    info!("Downloading {asset_name}...");
    download_asset(http, asset_url, token).await
}

/// Compute SHA256 of bytes and return hex string.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Extract the wshm binary from a tar.gz archive.
fn extract_from_targz(archive_data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let decoder = flate2::read::GzDecoder::new(archive_data);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "wshm" {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    anyhow::bail!("wshm binary not found in archive")
}

/// Extract the wshm.exe binary from a zip archive.
fn extract_from_zip(archive_data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let cursor = std::io::Cursor::new(archive_data);
    let mut zip = zip::ZipArchive::new(cursor)?;

    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        if file.name().ends_with("wshm.exe") || file.name() == "wshm" {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    anyhow::bail!("wshm binary not found in zip archive")
}

/// Replace the current binary atomically.
fn replace_binary(new_binary: &[u8]) -> Result<PathBuf> {
    let current_exe =
        std::env::current_exe().context("Cannot determine current executable path")?;

    // Detect package-managed installations — refuse to break metadata
    let path_str = current_exe.to_string_lossy();
    if path_str.contains("/Cellar/") || path_str.contains("/homebrew/") {
        anyhow::bail!(
            "Detected Homebrew installation ({}).\nUse 'brew upgrade wshm' instead to keep metadata consistent.",
            current_exe.display()
        );
    }
    if path_str.starts_with("/usr/bin/") {
        warn!(
            "{} may be managed by a package manager (apt/dnf/rpm). Consider using your package manager to upgrade.",
            current_exe.display()
        );
    }

    let parent = current_exe
        .parent()
        .context("Cannot determine binary directory")?;

    // Write to temp file first
    let tmp_path = parent.join(".wshm-update.tmp");
    let mut f = fs::File::create(&tmp_path)?;
    f.write_all(new_binary)?;
    f.flush()?;
    drop(f);

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755))?;
    }

    // Rename old binary to .bak (rollback point)
    let bak_path = parent.join(".wshm-old.bak");
    if bak_path.exists() {
        let _ = fs::remove_file(&bak_path);
    }
    fs::rename(&current_exe, &bak_path)
        .with_context(|| format!("Failed to backup current binary to {}", bak_path.display()))?;

    // Move new binary into place
    if let Err(e) = fs::rename(&tmp_path, &current_exe) {
        // Rollback: restore old binary
        warn!("Failed to install new binary, rolling back: {e}");
        let _ = fs::rename(&bak_path, &current_exe);
        anyhow::bail!("Update failed: {e}");
    }

    // Clean up backup
    let _ = fs::remove_file(&bak_path);

    Ok(current_exe)
}

/// Compare two semver version strings. Returns true if `remote` > `local`.
fn is_newer(remote_tag: &str, local: &str) -> bool {
    let remote = remote_tag.strip_prefix('v').unwrap_or(remote_tag);
    let parse = |s: &str| -> (u64, u64, u64) {
        let parts: Vec<u64> = s.split('.').filter_map(|p| p.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };
    parse(remote) > parse(local)
}

/// Check for updates and optionally apply them.
/// Returns Some(tag) if an update was applied, None if already up-to-date.
pub async fn check_and_update(apply: bool, json: bool) -> Result<Option<String>> {
    let http = reqwest::Client::new();
    let token = std::env::var("GITHUB_TOKEN").ok();
    let token_ref = token.as_deref();

    let (tag, url) = fetch_latest_release(&http, token_ref).await?;
    let remote_version = tag.strip_prefix('v').unwrap_or(&tag);
    let local_version = current_version();

    if !is_newer(&tag, local_version) {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "status": "up-to-date",
                    "current": local_version,
                    "latest": remote_version,
                })
            );
        } else {
            println!("Already up-to-date (v{local_version}).");
        }
        return Ok(None);
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": if apply { "updating" } else { "update-available" },
                "current": local_version,
                "latest": remote_version,
                "url": url,
            })
        );
    } else {
        println!("Update available: v{local_version} → v{remote_version}");
        if !apply {
            println!("Run `wshm update --apply` to install the update.");
            return Ok(None);
        }
    }

    if !apply {
        return Ok(None);
    }

    let target = asset_target()?;

    // Fetch release assets list (works for both public and private repos)
    let assets = fetch_release_assets(&http, &tag, token_ref).await?;

    // Download checksums
    let checksums = fetch_checksums(&http, &tag, token_ref, &assets).await?;
    let expected_hash = parse_checksum(&checksums, target)?;
    info!("Expected SHA256: {expected_hash}");

    // Download binary archive
    let archive_data = download_binary(&http, target, token_ref, &assets).await?;

    // Verify checksum of the archive
    let actual_hash = sha256_hex(&archive_data);
    if actual_hash != expected_hash {
        anyhow::bail!(
            "Checksum mismatch!\n  Expected: {expected_hash}\n  Got:      {actual_hash}\n\nBinary may have been tampered with. Aborting update."
        );
    }
    info!("Checksum verified: {actual_hash}");

    // Extract binary
    let binary = if target.contains("windows") {
        extract_from_zip(&archive_data)?
    } else {
        extract_from_targz(&archive_data)?
    };

    // Replace current binary
    let installed_path = replace_binary(&binary)?;

    // Store binary hash for startup integrity check
    store_binary_hash(&installed_path)?;

    if !json {
        println!(
            "Updated to v{remote_version} ({})",
            installed_path.display()
        );
        println!("SHA256: {actual_hash}");
    }

    // If running as systemd service, exit so systemd restarts us with the new binary
    if std::env::var("INVOCATION_ID").is_ok() {
        info!("Exiting for systemd restart with new binary...");
        std::process::exit(0);
    }

    Ok(Some(tag))
}

/// Silent background check, used by the daemon scheduler.
pub async fn auto_check_and_update() {
    match check_and_update(true, false).await {
        Ok(Some(tag)) => info!("Auto-updated to {tag}"),
        Ok(None) => info!("Auto-update check: already up-to-date"),
        Err(e) => warn!("Auto-update check failed: {e:#}"),
    }
}

// ── Binary integrity ─────────────────────────────────────────

/// Path where we store the expected SHA256 of the installed binary.
fn integrity_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(".wshm-binary.sha256")))
}

/// Store SHA256 of the binary after a successful update.
fn store_binary_hash(binary_path: &std::path::Path) -> Result<()> {
    let data = fs::read(binary_path).context("Failed to read binary for hash")?;
    let hash = sha256_hex(&data);
    if let Some(path) = integrity_path() {
        fs::write(&path, format!("{hash}  wshm\n"))?;
        info!("Stored binary integrity hash: {hash}");
    }
    Ok(())
}

/// Verify the running binary hasn't been modified since last update.
/// Returns Ok(true) if valid, Ok(false) if tampered, Err if no hash stored.
pub fn verify_binary_integrity() -> Result<bool> {
    let integrity = integrity_path().context("Cannot determine binary path")?;
    if !integrity.exists() {
        return Err(anyhow::anyhow!(
            "No integrity hash stored (first run or manual install)"
        ));
    }

    let stored = fs::read_to_string(&integrity)?;
    let expected = stored
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().next())
        .context("Invalid integrity file")?;

    let exe = std::env::current_exe().context("Cannot determine current exe")?;
    let data = fs::read(&exe).context("Cannot read current binary")?;
    let actual = sha256_hex(&data);

    if actual == expected {
        Ok(true)
    } else {
        warn!(
            "Binary integrity check FAILED!\n  Expected: {expected}\n  Actual:   {actual}\n  Path: {}",
            exe.display()
        );
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.2.0"));
    }

    #[test]
    fn test_parse_checksum() {
        let checksums = "abc123  wshm-x86_64-unknown-linux-gnu.tar.gz\ndef456  wshm-aarch64-apple-darwin.tar.gz\n";
        assert_eq!(
            parse_checksum(checksums, "x86_64-unknown-linux-gnu").unwrap(),
            "abc123"
        );
        assert_eq!(
            parse_checksum(checksums, "aarch64-apple-darwin").unwrap(),
            "def456"
        );
        assert!(parse_checksum(checksums, "windows").is_err());
    }

    #[test]
    fn test_sha256_hex() {
        let hash = sha256_hex(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
