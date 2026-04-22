use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const SERVICE_NAME: &str = "wshm";
const UNIT_PATH: &str = "/etc/systemd/system/wshm.service";

/// Generate the systemd unit file content.
#[allow(clippy::too_many_arguments)]
fn generate_unit(
    wshm_bin: &str,
    workdir: &str,
    repo: Option<&str>,
    apply: bool,
    poll: bool,
    poll_interval: u64,
    no_server: bool,
    bind: Option<&str>,
) -> String {
    let mut args = Vec::<String>::new();
    if let Some(r) = repo {
        // Validate repo format (owner/name only) to prevent injection
        if !r
            .chars()
            .all(|c| c.is_alphanumeric() || c == '/' || c == '-' || c == '_' || c == '.')
        {
            tracing::warn!("Suspicious repo name: {r} — skipping --repo flag");
        } else {
            args.push(format!("--repo {r}"));
        }
    }

    args.push("daemon".to_string());
    if apply {
        args.push("--apply".into());
    }
    if poll {
        args.push("--poll".into());
        args.push(format!("--poll-interval {poll_interval}"));
    }
    if no_server {
        args.push("--no-server".into());
    }
    if let Some(b) = bind {
        // Validate bind address format (IP:port) to prevent injection
        if b.parse::<std::net::SocketAddr>().is_ok() {
            args.push(format!("--bind {b}"));
        } else {
            tracing::warn!("Invalid bind address: {b} — skipping --bind flag");
        }
    }

    let exec_start = format!("{wshm_bin} {}", args.join(" "));

    let mut env_lines = String::new();
    // Pass RUST_LOG
    env_lines.push_str(
        "Environment=RUST_LOG=wshm=info
",
    );

    // Load credentials from .wshm/credentials in workdir
    let creds_path = Path::new(workdir).join(".wshm/credentials");
    if creds_path.exists() {
        env_lines.push_str(&format!(
            "EnvironmentFile=-{}
",
            creds_path.display()
        ));
    }

    // Note: We intentionally do NOT inline env var values into the unit file.
    // Secrets in unit files are world-readable via `systemctl show`.
    // Instead, use EnvironmentFile to load secrets at runtime.
    // Users should ensure their credentials are in .wshm/credentials or
    // create a dedicated /etc/wshm/env file with restrictive permissions (0600).

    format!(
        r#"[Unit]
Description=wshm — AI-powered GitHub agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={exec_start}
WorkingDirectory={workdir}
Restart=on-failure
RestartSec=10
{env_lines}
# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=wshm

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ReadWritePaths={workdir}
PrivateTmp=true

[Install]
WantedBy=multi-user.target
"#
    )
}

/// Find the wshm binary path.
fn find_wshm_binary() -> Result<String> {
    // Try current exe first
    if let Ok(exe) = std::env::current_exe() {
        return Ok(exe.to_string_lossy().to_string());
    }
    // Fallback: which wshm
    let output = Command::new("which")
        .arg("wshm")
        .output()
        .context("Failed to find wshm binary")?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        anyhow::bail!("Cannot find wshm binary. Install it to /usr/local/bin/wshm first.")
    }
}

pub fn install(args: &crate::cli::DaemonArgs) -> Result<()> {
    // Must be root
    if !is_root() {
        anyhow::bail!("systemd install requires root. Run with sudo.");
    }

    let wshm_bin = find_wshm_binary()?;
    let workdir = match &args.workdir {
        Some(w) => PathBuf::from(w),
        None => std::env::current_dir()?,
    };
    let workdir_str = workdir.to_string_lossy();

    // Ensure workdir has .wshm/ or config
    if !workdir.join(".wshm").exists() && !workdir.join(".wshm/config.toml").exists() {
        println!("Warning: {workdir_str}/.wshm/ not found. The daemon will use defaults.");
    }

    let unit = generate_unit(
        &wshm_bin,
        &workdir_str,
        args.repo.as_deref(),
        args.apply,
        args.poll,
        args.poll_interval,
        args.no_server,
        args.bind.as_deref(),
    );

    // Write unit file
    std::fs::write(UNIT_PATH, &unit).with_context(|| format!("Failed to write {UNIT_PATH}"))?;

    println!("Wrote {UNIT_PATH}");
    println!();
    println!("{unit}");

    // Reload systemd
    run_cmd("systemctl", &["daemon-reload"])?;
    println!("Reloaded systemd.");

    // Enable and start
    run_cmd("systemctl", &["enable", SERVICE_NAME])?;
    println!("Enabled {SERVICE_NAME} service.");

    run_cmd("systemctl", &["start", SERVICE_NAME])?;
    println!("Started {SERVICE_NAME} service.");

    println!();
    println!("Done! Check status with:");
    println!("  systemctl status {SERVICE_NAME}");
    println!("  journalctl -u {SERVICE_NAME} -f");

    Ok(())
}

pub fn uninstall() -> Result<()> {
    if !is_root() {
        anyhow::bail!("systemd uninstall requires root. Run with sudo.");
    }

    let unit_path = Path::new(UNIT_PATH);

    if !unit_path.exists() {
        println!("Service not installed ({UNIT_PATH} not found).");
        return Ok(());
    }

    // Stop the service (ignore errors if not running)
    let _ = run_cmd("systemctl", &["stop", SERVICE_NAME]);
    println!("Stopped {SERVICE_NAME}.");

    // Disable
    let _ = run_cmd("systemctl", &["disable", SERVICE_NAME]);
    println!("Disabled {SERVICE_NAME}.");

    // Remove unit file
    std::fs::remove_file(unit_path).with_context(|| format!("Failed to remove {UNIT_PATH}"))?;
    println!("Removed {UNIT_PATH}.");

    // Reload systemd
    run_cmd("systemctl", &["daemon-reload"])?;
    println!("Reloaded systemd.");

    println!();
    println!("{SERVICE_NAME} service uninstalled.");

    Ok(())
}

fn run_cmd(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("Failed to run {cmd} {}", args.join(" ")))?;
    if !status.success() {
        anyhow::bail!("{cmd} {} failed with {status}", args.join(" "));
    }
    Ok(())
}

fn is_root() -> bool {
    // Check UID via /proc or id command to avoid unsafe FFI
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            } else {
                None
            }
        })
        .map(|uid| uid == 0)
        .unwrap_or(false)
}
