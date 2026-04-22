use anyhow::{Context, Result};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::cli::LoginArgs;

/// Credentials file location: `.wshm/credentials`
/// This file is gitignored and never committed.
fn credentials_path() -> PathBuf {
    PathBuf::from(".wshm").join("credentials")
}

/// Read a line from stdin (for non-sensitive input).
fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

/// Read a secret from stdin (no echo).
fn read_secret(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush()?;
    // Try rpassword-style: disable echo on tty
    // Fallback: just read normally (CI environments)
    let input = if atty_is_tty() {
        disable_echo();
        let mut s = String::new();
        io::stdin().read_line(&mut s)?;
        enable_echo();
        println!(); // newline after hidden input
        s
    } else {
        let mut s = String::new();
        io::stdin().read_line(&mut s)?;
        s
    };
    Ok(input.trim().to_string())
}

fn atty_is_tty() -> bool {
    use is_terminal::IsTerminal;
    std::io::stdin().is_terminal()
}

#[cfg(unix)]
fn disable_echo() {
    let _ = std::process::Command::new("stty").arg("-echo").status();
}

#[cfg(unix)]
fn enable_echo() {
    let _ = std::process::Command::new("stty").arg("echo").status();
}

#[cfg(not(unix))]
fn disable_echo() {}

#[cfg(not(unix))]
fn enable_echo() {}

/// Load existing credentials from .wshm/credentials
fn load_credentials() -> std::collections::HashMap<String, String> {
    let path = credentials_path();
    if !path.exists() {
        return std::collections::HashMap::new();
    }
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "Warning: could not read credentials file {}: {e}",
                path.display()
            );
            return std::collections::HashMap::new();
        }
    };
    let mut map = std::collections::HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

/// Save credentials to .wshm/credentials
fn save_credentials(creds: &std::collections::HashMap<String, String>) -> Result<()> {
    let path = credentials_path();
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Credentials path has no parent directory"))?;
    fs::create_dir_all(parent)?;

    let mut content =
        String::from("# wshm credentials — DO NOT COMMIT\n# This file is in .gitignore\n\n");
    let mut keys: Vec<&String> = creds.keys().collect();
    keys.sort();
    for key in keys {
        content.push_str(&format!("{}={}\n", key, creds[key]));
    }

    // Write with restrictive permissions from the start on Unix
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        f.write_all(content.as_bytes())?;
    }

    #[cfg(not(unix))]
    {
        fs::write(&path, &content)?;
    }

    Ok(())
}

/// Ensure .wshm/credentials is in .gitignore.
/// Logs warnings on failure — credentials could leak if gitignore is not set.
fn ensure_gitignore() {
    let gitignore = Path::new(".wshm/.gitignore");
    if gitignore.exists() {
        let content = match fs::read_to_string(gitignore) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Warning: could not read {}: {e}", gitignore.display());
                return;
            }
        };
        // Check for exact line match (not substring) to avoid false positives
        let has_credentials = content.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == "credentials" || trimmed == "/credentials"
        });
        if has_credentials {
            return;
        }
        if let Err(e) = fs::write(gitignore, format!("{content}\ncredentials\n")) {
            eprintln!(
                "Warning: could not update {}: {e} — credentials may not be gitignored",
                gitignore.display()
            );
        }
    } else {
        if let Err(e) = fs::create_dir_all(".wshm") {
            eprintln!("Warning: could not create .wshm/: {e}");
            return;
        }
        if let Err(e) = fs::write(gitignore, "logs/\ncredentials\n") {
            eprintln!(
                "Warning: could not create {}: {e} — credentials may not be gitignored",
                gitignore.display()
            );
        }
    }
}

pub fn run(args: &LoginArgs) -> Result<()> {
    if args.status {
        return show_status();
    }

    let do_all = !args.github && !args.ai && !args.claude && !args.license;

    ensure_gitignore();

    if do_all || args.github {
        login_github()?;
    }

    if args.claude {
        login_claude()?;
    } else if do_all || args.ai {
        login_ai()?;
    }

    println!("\nAll set. Run `wshm daemon --apply` to start.");
    Ok(())
}

fn login_github() -> Result<()> {
    println!("── GitHub Authentication ──\n");

    // Check if gh CLI is available
    let gh_available = std::process::Command::new("gh")
        .arg("--version")
        .output()
        .is_ok();

    if gh_available {
        // Check if already logged in
        let token_output = std::process::Command::new("gh")
            .args(["auth", "token"])
            .output();

        if let Ok(output) = token_output {
            if output.status.success() {
                let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !token.is_empty() {
                    // Get username
                    let user = std::process::Command::new("gh")
                        .args(["api", "user", "--jq", ".login"])
                        .output()
                        .ok()
                        .and_then(|o| {
                            if o.status.success() {
                                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| "unknown".to_string());

                    println!("Already authenticated as {user} via `gh auth`.");
                    let reauth = read_line("Re-authenticate? [y/N] ")?;
                    if reauth.to_lowercase() != "y" {
                        return Ok(());
                    }
                }
            }
        }

        println!("Launching `gh auth login`...\n");
        let status = std::process::Command::new("gh")
            .args(["auth", "login", "--web"])
            .status()
            .context("Failed to run gh auth login")?;

        if !status.success() {
            // Fallback to manual token
            println!("\ngh auth login failed. Enter a token manually.");
            return login_github_manual();
        }

        println!("GitHub authentication successful.");
    } else {
        println!("`gh` CLI not found. Enter a GitHub token manually.");
        println!("Create one at: https://github.com/settings/tokens");
        println!("Required scopes: repo, read:org\n");
        login_github_manual()?;
    }

    Ok(())
}

fn login_github_manual() -> Result<()> {
    let token = read_secret("GitHub token: ")?;
    if token.is_empty() {
        anyhow::bail!("No token provided");
    }

    let mut creds = load_credentials();
    creds.insert("GITHUB_TOKEN".to_string(), token);
    save_credentials(&creds)?;

    println!("GitHub token saved to .wshm/credentials");
    Ok(())
}

fn login_ai() -> Result<()> {
    println!("\n── AI Provider Authentication ──\n");

    let providers = [
        (
            "anthropic",
            "ANTHROPIC_API_KEY",
            "https://console.anthropic.com/settings/keys",
        ),
        (
            "openai",
            "OPENAI_API_KEY",
            "https://platform.openai.com/api-keys",
        ),
        (
            "google",
            "GOOGLE_API_KEY",
            "https://aistudio.google.com/apikey",
        ),
        (
            "mistral",
            "MISTRAL_API_KEY",
            "https://console.mistral.ai/api-keys",
        ),
        ("groq", "GROQ_API_KEY", "https://console.groq.com/keys"),
        (
            "deepseek",
            "DEEPSEEK_API_KEY",
            "https://platform.deepseek.com/api_keys",
        ),
        ("xai", "XAI_API_KEY", "https://console.x.ai"),
        ("ollama", "", ""),
    ];

    println!("Available providers:");
    for (i, (name, _, _)) in providers.iter().enumerate() {
        let marker = if i == 0 { " (default)" } else { "" };
        println!("  {}: {name}{marker}", i + 1);
    }

    let choice = read_line("\nProvider [1]: ")?;
    let idx = if choice.is_empty() {
        0
    } else {
        choice.parse::<usize>().unwrap_or(1).saturating_sub(1)
    };

    let (name, env_var, url) = providers.get(idx).unwrap_or(&providers[0]);

    if *name == "ollama" {
        println!("Ollama runs locally — no API key needed.");
        println!("Make sure Ollama is running: `ollama serve`");
        return Ok(());
    }

    println!("\nGet your {name} API key at: {url}\n");

    let key = read_secret(&format!("{env_var}: "))?;
    if key.is_empty() {
        anyhow::bail!("No API key provided");
    }

    let mut creds = load_credentials();
    creds.insert(env_var.to_string(), key);
    save_credentials(&creds)?;

    println!("{name} API key saved to .wshm/credentials");
    Ok(())
}

fn login_claude() -> Result<()> {
    println!("\n── Claude OAuth (Max/Pro/Team) ──\n");

    // Check if claude CLI is available
    let claude_available = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if claude_available {
        println!("Found `claude` CLI. Launching OAuth login...\n");
        let status = std::process::Command::new("claude")
            .arg("login")
            .status()
            .context("Failed to run claude login")?;

        if status.success() {
            // Read the token from ~/.claude/.credentials.json
            if let Some(token) = read_claude_oauth_token() {
                let mut creds = load_credentials();
                creds.insert("ANTHROPIC_OAUTH_TOKEN".to_string(), token);
                save_credentials(&creds)?;
                println!("Claude OAuth token saved. Your Max/Pro subscription will be used.");
                return Ok(());
            }
            println!("Claude login succeeded but could not read token. Using claude CLI directly.");
            return Ok(());
        }
        println!("claude login failed.");
    }

    // Fallback: manual OAuth via device flow is not publicly available
    // Offer API key as alternative
    println!("`claude` CLI not found. Install it first:");
    println!("  npm install -g @anthropic-ai/claude-code");
    println!("  claude login\n");
    println!("Or use an API key instead:");
    login_ai()?;
    Ok(())
}

/// Read OAuth access token from ~/.claude/.credentials.json
fn read_claude_oauth_token() -> Option<String> {
    // Check CLAUDE_CREDENTIALS_JSON env first (CI)
    if let Ok(json_str) = std::env::var("CLAUDE_CREDENTIALS_JSON") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_str) {
            if let Some(token) = extract_oauth_token(&v) {
                return Some(token);
            }
        }
    }

    // Check ~/.claude/.credentials.json
    let home = dirs::home_dir()?;
    let creds_path = home.join(".claude").join(".credentials.json");
    let content = fs::read_to_string(&creds_path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    extract_oauth_token(&v)
}

fn extract_oauth_token(v: &serde_json::Value) -> Option<String> {
    // Format: {"claudeAiOauth": {"accessToken": "..."}}
    v.get("claudeAiOauth")
        .and_then(|o| o.get("accessToken"))
        .and_then(|t| t.as_str())
        .map(String::from)
        .or_else(|| {
            // Alternative format: {"accessToken": "..."}
            v.get("accessToken")
                .and_then(|t| t.as_str())
                .map(String::from)
        })
}

/// Check if the `claude` CLI is installed and accessible.
fn is_claude_cli_available() -> bool {
    std::process::Command::new("claude")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve the best Anthropic auth: OAuth token (Max/Pro) > API key
pub fn resolve_anthropic_auth() -> Option<(String, bool)> {
    // Priority 1: OAuth token from .wshm/credentials
    let creds = load_credentials();
    if let Some(token) = creds.get("ANTHROPIC_OAUTH_TOKEN") {
        if !token.is_empty() {
            return Some((token.clone(), true)); // (token, is_oauth)
        }
    }

    // Priority 2: OAuth token from env
    if let Ok(token) = std::env::var("ANTHROPIC_OAUTH_TOKEN") {
        if !token.is_empty() {
            return Some((token, true));
        }
    }

    // Priority 3: OAuth from ~/.claude/.credentials.json
    if let Some(token) = read_claude_oauth_token() {
        return Some((token, true));
    }

    // Priority 4: API key
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            return Some((key, false));
        }
    }

    // Priority 5: claude CLI available (Max/Pro/Team — no token needed, uses `claude -p`)
    if is_claude_cli_available() {
        return Some(("claude-cli".to_string(), true));
    }

    None
}

fn show_status() -> Result<()> {
    println!("── Authentication Status ──\n");

    // GitHub
    let gh_token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("WSHM_TOKEN"))
        .ok();

    let gh_cli = std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        });

    let creds = load_credentials();
    let creds_gh = creds.get("GITHUB_TOKEN");

    print!("GitHub: ");
    if gh_token.is_some() {
        println!("authenticated (env var)");
    } else if gh_cli.is_some() {
        println!("authenticated (gh CLI)");
    } else if creds_gh.is_some() {
        println!("authenticated (.wshm/credentials)");
    } else {
        println!("not configured");
    }

    // Claude OAuth
    print!("Claude: ");
    if creds.contains_key("ANTHROPIC_OAUTH_TOKEN") {
        println!("authenticated (OAuth Max/Pro)");
    } else if read_claude_oauth_token().is_some() {
        println!("authenticated (~/.claude credentials)");
    } else {
        println!("not configured (run `wshm login --claude`)");
    }

    // AI API key
    let ai_vars = [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GOOGLE_API_KEY",
        "MISTRAL_API_KEY",
        "GROQ_API_KEY",
        "DEEPSEEK_API_KEY",
        "XAI_API_KEY",
    ];

    print!("AI key: ");
    let from_env: Vec<&&str> = ai_vars
        .iter()
        .filter(|v| std::env::var(v).is_ok())
        .collect();
    let from_creds: Vec<&&str> = ai_vars.iter().filter(|v| creds.contains_key(**v)).collect();

    if !from_env.is_empty() {
        println!("authenticated via {} (env var)", from_env[0]);
    } else if !from_creds.is_empty() {
        println!("authenticated via {} (.wshm/credentials)", from_creds[0]);
    } else {
        println!("not configured");
    }

    // Webhook secret
    print!("Webhook: ");
    if std::env::var("WSHM_WEBHOOK_SECRET").is_ok() {
        println!("configured (env var)");
    } else if creds.contains_key("WSHM_WEBHOOK_SECRET") {
        println!("configured (.wshm/credentials)");
    } else {
        println!("not configured (optional)");
    }

    Ok(())
}

/// Load credentials from .wshm/credentials and inject into env vars.
/// Called at startup before Config::load so that tokens are available.
pub fn inject_credentials() {
    let creds = load_credentials();
    for (key, value) in &creds {
        // Only set if not already set (env var takes precedence)
        if std::env::var(key).is_err() {
            std::env::set_var(key, value);
        }
    }
}
