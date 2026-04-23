use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tracing::{info, warn};

/// Per-binary update configuration. Passed to all update functions so the
/// same code serves both the OSS binary and wshm-pro without duplication.
#[derive(Copy, Clone)]
pub struct UpdateConfig {
    /// Short binary name, e.g. `"wshm"` or `"wshm-pro"`.
    pub binary_name: &'static str,
    /// GitHub repo that hosts release assets, e.g. `"wshm-dev/homebrew-tap"`.
    pub repo: &'static str,
    /// Version suffix to strip before semver comparison, e.g. `Some("-pro")`.
    pub version_suffix: Option<&'static str>,
}

impl UpdateConfig {
    pub const fn oss() -> Self {
        Self {
            binary_name: "wshm",
            repo: "wshm-dev/homebrew-tap",
            version_suffix: None,
        }
    }

    pub const fn pro() -> Self {
        Self {
            binary_name: "wshm-pro",
            repo: "wshm-dev/homebrew-tap",
            version_suffix: Some("-pro"),
        }
    }
}

/// Current version from Cargo.toml
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

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

async fn fetch_latest_release(
    http: &reqwest::Client,
    repo: &str,
    binary_name: &str,
    token: Option<&str>,
) -> Result<(String, String)> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");

    let mut req = http
        .get(&url)
        .header("User-Agent", format!("{binary_name}-updater"))
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

async fn fetch_release_assets(
    http: &reqwest::Client,
    repo: &str,
    binary_name: &str,
    tag: &str,
    token: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");

    let mut req = http
        .get(&url)
        .header("User-Agent", format!("{binary_name}-updater"))
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

async fn download_asset(
    http: &reqwest::Client,
    binary_name: &str,
    api_url: &str,
    token: Option<&str>,
) -> Result<Vec<u8>> {
    let mut req = http
        .get(api_url)
        .header("User-Agent", format!("{binary_name}-updater"))
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

async fn fetch_checksums(
    http: &reqwest::Client,
    binary_name: &str,
    tag: &str,
    token: Option<&str>,
    assets: &[(String, String)],
) -> Result<String> {
    let checksums_name = format!("checksums-{tag}.sha256");
    let asset_url = assets
        .iter()
        .find(|(name, _)| name == &checksums_name || name == "checksums.txt")
        .map(|(_, url)| url.as_str())
        .with_context(|| format!("No checksums file found in release {tag}"))?;

    let data = download_asset(http, binary_name, asset_url, token).await?;
    String::from_utf8(data).context("Checksums file is not valid UTF-8")
}

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

async fn download_binary(
    http: &reqwest::Client,
    binary_name: &str,
    target: &str,
    token: Option<&str>,
    assets: &[(String, String)],
) -> Result<Vec<u8>> {
    let ext = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    let asset_name = format!("{binary_name}-{target}.{ext}");

    let asset_url = assets
        .iter()
        .find(|(name, _)| name == &asset_name)
        .map(|(_, url)| url.as_str())
        .with_context(|| format!("Asset {asset_name} not found in release"))?;

    info!("Downloading {asset_name}...");
    download_asset(http, binary_name, asset_url, token).await
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn extract_from_targz(binary_name: &str, archive_data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let decoder = flate2::read::GzDecoder::new(archive_data);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == binary_name {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    anyhow::bail!("{binary_name} binary not found in archive")
}

fn extract_from_zip(binary_name: &str, archive_data: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let cursor = std::io::Cursor::new(archive_data);
    let mut zip = zip::ZipArchive::new(cursor)?;

    let exe_name = format!("{binary_name}.exe");
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let file_name = file.name().to_string();
        if file_name.ends_with(&exe_name) || file_name == binary_name {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    anyhow::bail!("{binary_name} binary not found in zip archive")
}

fn replace_binary(binary_name: &str, new_binary: &[u8]) -> Result<PathBuf> {
    let current_exe =
        std::env::current_exe().context("Cannot determine current executable path")?;

    let path_str = current_exe.to_string_lossy();
    if path_str.contains("/Cellar/") || path_str.contains("/homebrew/") {
        anyhow::bail!(
            "Detected Homebrew installation ({}).\nUse 'brew upgrade {binary_name}' instead to keep metadata consistent.",
            current_exe.display()
        );
    }
    if path_str.starts_with("/usr/bin/") {
        warn!(
            "{} may be managed by a package manager. Consider using your package manager to upgrade.",
            current_exe.display()
        );
    }

    let parent = current_exe
        .parent()
        .context("Cannot determine binary directory")?;

    let tmp_path = parent.join(format!(".{binary_name}-update.tmp"));
    let mut f = fs::File::create(&tmp_path)?;
    f.write_all(new_binary)?;
    f.flush()?;
    drop(f);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o755))?;
    }

    let bak_path = parent.join(format!(".{binary_name}-old.bak"));
    if bak_path.exists() {
        let _ = fs::remove_file(&bak_path);
    }
    fs::rename(&current_exe, &bak_path)
        .with_context(|| format!("Failed to backup current binary to {}", bak_path.display()))?;

    if let Err(e) = fs::rename(&tmp_path, &current_exe) {
        warn!("Failed to install new binary, rolling back: {e}");
        let _ = fs::rename(&bak_path, &current_exe);
        anyhow::bail!("Update failed: {e}");
    }

    let _ = fs::remove_file(&bak_path);

    Ok(current_exe)
}

fn strip_version<'a>(s: &'a str, suffix: Option<&str>) -> &'a str {
    let s = s.strip_prefix('v').unwrap_or(s);
    if let Some(suf) = suffix {
        s.strip_suffix(suf).unwrap_or(s)
    } else {
        s
    }
}

fn parse_version(s: &str) -> (u64, u64, u64) {
    let parts: Vec<u64> = s.split('.').filter_map(|p| p.parse().ok()).collect();
    (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    )
}

fn is_newer(cfg: &UpdateConfig, remote_tag: &str, local: &str) -> bool {
    parse_version(strip_version(remote_tag, cfg.version_suffix))
        > parse_version(strip_version(local, cfg.version_suffix))
}

fn integrity_path(binary_name: &str) -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| {
        p.parent()
            .map(|d| d.join(format!(".{binary_name}-binary.sha256")))
    })
}

fn store_binary_hash(binary_name: &str, binary_path: &std::path::Path) -> Result<()> {
    let data = fs::read(binary_path).context("Failed to read binary for hash")?;
    let hash = sha256_hex(&data);
    if let Some(path) = integrity_path(binary_name) {
        fs::write(&path, format!("{hash}  {binary_name}\n"))?;
        info!("Stored binary integrity hash: {hash}");
    }
    Ok(())
}

/// Check for updates and optionally apply them.
/// Returns `Some(tag)` if an update was applied, `None` if already up-to-date.
pub async fn check_and_update(
    cfg: &UpdateConfig,
    apply: bool,
    json: bool,
) -> Result<Option<String>> {
    let http = reqwest::Client::new();
    let token = std::env::var("GITHUB_TOKEN").ok();
    let token_ref = token.as_deref();

    let (tag, url) = fetch_latest_release(&http, cfg.repo, cfg.binary_name, token_ref).await?;
    let remote_version = tag.strip_prefix('v').unwrap_or(&tag);
    let local_version = current_version();

    if !is_newer(cfg, &tag, local_version) {
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
            println!(
                "Run `{} update --apply` to install the update.",
                cfg.binary_name
            );
            return Ok(None);
        }
    }

    if !apply {
        return Ok(None);
    }

    let target = asset_target()?;
    let assets = fetch_release_assets(&http, cfg.repo, cfg.binary_name, &tag, token_ref).await?;
    let checksums = fetch_checksums(&http, cfg.binary_name, &tag, token_ref, &assets).await?;
    let expected_hash = parse_checksum(&checksums, target)?;
    info!("Expected SHA256: {expected_hash}");

    let archive_data = download_binary(&http, cfg.binary_name, target, token_ref, &assets).await?;

    let actual_hash = sha256_hex(&archive_data);
    if actual_hash != expected_hash {
        anyhow::bail!(
            "Checksum mismatch!\n  Expected: {expected_hash}\n  Got:      {actual_hash}\n\nBinary may have been tampered with. Aborting update."
        );
    }
    info!("Checksum verified: {actual_hash}");

    let binary = if target.contains("windows") {
        extract_from_zip(cfg.binary_name, &archive_data)?
    } else {
        extract_from_targz(cfg.binary_name, &archive_data)?
    };

    let installed_path = replace_binary(cfg.binary_name, &binary)?;
    store_binary_hash(cfg.binary_name, &installed_path)?;

    if !json {
        println!(
            "Updated to v{remote_version} ({})",
            installed_path.display()
        );
        println!("SHA256: {actual_hash}");
    }

    if std::env::var("INVOCATION_ID").is_ok() {
        info!("Exiting for systemd restart with new binary...");
        std::process::exit(0);
    }

    Ok(Some(tag))
}

/// Silent background update, used by the daemon scheduler.
pub async fn auto_check_and_update(cfg: &UpdateConfig) {
    match check_and_update(cfg, true, false).await {
        Ok(Some(tag)) => info!("Auto-updated {} to {tag}", cfg.binary_name),
        Ok(None) => info!("Auto-update check: {} already up-to-date", cfg.binary_name),
        Err(e) => warn!("Auto-update check failed: {e:#}"),
    }
}

/// Return a JSON status object for the update check (used by web API).
pub async fn check_update_status(cfg: &UpdateConfig) -> Result<serde_json::Value> {
    let http = reqwest::Client::new();
    let token = std::env::var("GITHUB_TOKEN").ok();
    let token_ref = token.as_deref();

    let (tag, url) = fetch_latest_release(&http, cfg.repo, cfg.binary_name, token_ref).await?;
    let remote_version = tag.strip_prefix('v').unwrap_or(&tag);
    let local_version = current_version();

    let status = if is_newer(cfg, &tag, local_version) {
        "update-available"
    } else {
        "up-to-date"
    };

    Ok(serde_json::json!({
        "status": status,
        "current": local_version,
        "latest": remote_version,
        "url": url,
    }))
}

// ── Binary integrity ─────────────────────────────────────────

/// Verify the running binary hasn't been modified since last update.
/// Returns `Ok(true)` if valid, `Ok(false)` if tampered, `Err` if no hash stored.
pub fn verify_binary_integrity(cfg: &UpdateConfig) -> Result<bool> {
    let integrity = integrity_path(cfg.binary_name).context("Cannot determine binary path")?;
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
    fn test_is_newer_oss() {
        let cfg = UpdateConfig::oss();
        assert!(is_newer(&cfg, "v0.2.0", "0.1.0"));
        assert!(is_newer(&cfg, "v1.0.0", "0.9.9"));
        assert!(is_newer(&cfg, "0.1.1", "0.1.0"));
        assert!(!is_newer(&cfg, "v0.1.0", "0.1.0"));
        assert!(!is_newer(&cfg, "v0.1.0", "0.2.0"));
    }

    #[test]
    fn test_is_newer_pro() {
        let cfg = UpdateConfig::pro();
        assert!(is_newer(&cfg, "v0.28.1-pro", "0.28.0-pro"));
        assert!(is_newer(&cfg, "v1.0.0-pro", "0.9.9-pro"));
        assert!(!is_newer(&cfg, "v0.28.1-pro", "0.28.1-pro"));
        assert!(!is_newer(&cfg, "v0.28.0-pro", "0.28.1-pro"));
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
