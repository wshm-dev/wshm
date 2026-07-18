use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::{BackupArgs, RestoreArgs};

/// Create a backup of .wshm/ directory (state.db, config.toml, credentials).
pub fn backup(args: &BackupArgs) -> Result<()> {
    let wshm_dir = PathBuf::from(".wshm");
    if !wshm_dir.exists() {
        anyhow::bail!(".wshm/ directory not found. Run `wshm config init` first.");
    }

    let timestamp = chrono::Local::now().format("%Y-%m-%d_%H%M%S");
    let output = match &args.output {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(format!(".wshm/backup-{timestamp}.tar.gz")),
    };

    // Collect files to backup
    let mut files: Vec<PathBuf> = Vec::new();

    // Always backup: state.db, config.toml, credentials
    for name in &["state.db", "config.toml", "credentials"] {
        let path = wshm_dir.join(name);
        if path.exists() {
            files.push(path);
        }
    }

    // Optional: logs
    if args.include_logs {
        let logs_dir = wshm_dir.join("logs");
        if logs_dir.exists() {
            collect_files_recursive(&logs_dir, &mut files)?;
        }
    }

    if files.is_empty() {
        anyhow::bail!("No files to backup in .wshm/");
    }

    // Create tar.gz
    let file =
        fs::File::create(&output).with_context(|| format!("Cannot create {}", output.display()))?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);

    for path in &files {
        let archive_name = path.strip_prefix(".").unwrap_or(path);
        tar.append_path_with_name(path, archive_name)
            .with_context(|| format!("Failed to add {} to backup", path.display()))?;
    }

    tar.finish()?;

    let size = fs::metadata(&output)?.len();
    let size_str = if size > 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", size as f64 / 1024.0)
    };

    println!("Backup created: {} ({})", output.display(), size_str);
    println!("  Files: {}", files.len());
    println!("  Restore with: wshm restore {}", output.display());

    Ok(())
}

/// Restore .wshm/ from a backup file.
pub fn restore(args: &RestoreArgs) -> Result<()> {
    let backup_path = Path::new(&args.file);
    if !backup_path.exists() {
        anyhow::bail!("Backup file not found: {}", args.file);
    }

    let wshm_dir = PathBuf::from(".wshm");

    // Safety check
    if wshm_dir.join("state.db").exists() && !args.force {
        println!("WARNING: .wshm/state.db already exists.");
        println!("This will overwrite your current database.");
        println!("Use --force to confirm, or backup first with `wshm backup`.");
        anyhow::bail!("Restore aborted. Use --force to overwrite.");
    }

    // Extract tar.gz
    let file = fs::File::open(backup_path)
        .with_context(|| format!("Cannot open {}", backup_path.display()))?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(dec);

    // Resolve the canonical destination root once. Every extracted file
    // must end up strictly inside this directory.
    fs::create_dir_all(&wshm_dir)?;
    let canonical_root = wshm_dir
        .canonicalize()
        .with_context(|| format!("Cannot resolve {}", wshm_dir.display()))?;

    let mut restored = 0u32;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();

        // Security: reject Zip-Slip style entries. An absolute path or any
        // `..` component lets the entry escape .wshm/ even when its declared
        // prefix looks safe (e.g. ".wshm/../../etc/cron.d/evil").
        use std::path::Component;
        if path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            tracing::warn!("Skipping unsafe archive entry: {}", path.display());
            continue;
        }

        // Security: only extract into .wshm/
        if !path.starts_with("wshm/") && !path.starts_with(".wshm/") {
            tracing::warn!("Skipping file outside .wshm/: {}", path.display());
            continue;
        }

        // Create parent dirs
        let target = PathBuf::from(".").join(&path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        // Defense in depth: after creating the parent dirs, verify the
        // resolved parent is still inside the canonical .wshm/ root before
        // writing anything. This catches symlink-based escapes too.
        if let Some(parent) = target.parent() {
            let canonical_parent = parent
                .canonicalize()
                .with_context(|| format!("Cannot resolve {}", parent.display()))?;
            if !canonical_parent.starts_with(&canonical_root) {
                tracing::warn!(
                    "Skipping entry resolving outside .wshm/: {}",
                    path.display()
                );
                continue;
            }
        }

        entry
            .unpack(&target)
            .with_context(|| format!("Failed to extract {}", path.display()))?;
        restored += 1;
    }

    println!("Restored {restored} files from {}", backup_path.display());

    // Set permissions on credentials
    let creds = wshm_dir.join("credentials");
    if creds.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&creds, fs::Permissions::from_mode(0o600)) {
                tracing::warn!(
                    "Failed to secure restored credentials file {}: {e}; the secret may be world-readable",
                    creds.display()
                );
            }
        }
    }

    println!("Done. Run `wshm sync` to refresh from GitHub.");

    Ok(())
}

fn collect_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                collect_files_recursive(&path, files)?;
            } else {
                files.push(path);
            }
        }
    }
    Ok(())
}
