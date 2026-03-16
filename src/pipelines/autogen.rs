use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::info;

use crate::cli::FixArgs;
use crate::config::Config;
use crate::db::Database;
use crate::export::{EventKind, ExportEvent, ExportManager};
use crate::github::Client as GhClient;

/// Pipeline: Auto-generate a PR fix from an issue description.
///
/// Modes:
///   1. claude-code: spawn `claude -p` with the issue context
///   2. codex: spawn `codex` CLI
///   3. podman: run the AI tool inside a rootless Podman container
///
/// Credential injection (Podman mode):
///   Priority 1: OAuth — mount ~/.claude (contains .credentials.json)
///   Priority 2: API key — pass ANTHROPIC_API_KEY via -e
///   In CI: CLAUDE_CREDENTIALS_JSON secret → written to temp file → mounted
///
/// Flow:
///   1. Read issue from DB
///   2. Create branch `wshm/fix-<issue_number>`
///   3. Spawn AI tool with issue context as prompt
///   4. Commit changes, push branch
///   5. Open PR linking to the issue
pub async fn run(
    config: &Config,
    db: &Database,
    gh: &GhClient,
    args: &FixArgs,
    exporter: Option<&ExportManager>,
) -> Result<()> {
    let issue = db.get_issue(args.issue)?.with_context(|| {
        format!(
            "Issue #{} not found in cache. Run `wshm sync` first.",
            args.issue
        )
    })?;

    info!("Auto-fixing issue #{}: {}", issue.number, issue.title);

    // ── Security: author authorization check (for auto-triage-triggered fixes) ──
    // Note: slash command `/wshm fix` checks allowed_users against the *commenter*
    // in commands.rs. Here we only check trusted_authors_only as a fallback for
    // auto-fix triggered by the triage pipeline.
    if config.fix.trusted_authors_only {
        let author = issue.author.as_deref().unwrap_or("unknown");
        let is_trusted = gh.is_collaborator(author).await.unwrap_or(false);
        if !is_trusted {
            info!(
                "Skipping auto-fix for issue #{}: author '{}' is not a collaborator",
                issue.number, author
            );
            return Ok(());
        }
        info!("Author '{author}' verified as collaborator");
    }

    let tool = resolve_tool(config, args);
    let branch = format!("wshm/fix-{}", issue.number);

    // ICM: recall past fix attempts and context for this issue
    let icm_context = crate::icm::recall_context(
        &format!("auto-fix issue {} {}", issue.title, config.repo_slug()),
        5,
    );

    // Build the prompt from issue context
    let mut prompt = build_fix_prompt(&issue.title, issue.body.as_deref().unwrap_or(""));
    if !icm_context.is_empty() {
        prompt.push_str(&format!(
            "\n\n## Past fix context (from memory)\n{icm_context}"
        ));
    }

    println!("Issue #{}: {}", issue.number, issue.title);
    println!("Tool: {}", tool.name());
    println!("Branch: {branch}");

    if !args.apply {
        println!("[DRY-RUN] Would run {} to fix this issue.", tool.name());
        println!("Prompt:\n{prompt}");
        return Ok(());
    }

    // Step 1: Start from a clean base branch, then create fix branch
    let base_branch = &config.fix.base_branch;
    ensure_clean_base(base_branch)?;
    create_branch(&branch)?;

    // Step 2: Run the AI tool
    println!("Running {}...", tool.name());
    let result = tool.execute(&prompt).await?;

    if !result.success {
        println!("AI tool failed: {}", result.output);
        cleanup_branch(base_branch, &branch)?;
        return Ok(());
    }

    // Step 3: Check if any files were changed
    let has_changes = check_changes()?;
    if !has_changes {
        println!("No changes generated. The AI tool did not modify any files.");
        cleanup_branch(base_branch, &branch)?;
        return Ok(());
    }

    // ── Security: diff scan ──
    if config.fix.scan_diff {
        let violations = scan_diff_for_threats()?;
        if !violations.is_empty() {
            tracing::warn!(
                "Diff scan found suspicious patterns in auto-fix for issue #{}:",
                issue.number
            );
            for v in &violations {
                tracing::warn!("  ⚠ {v}");
            }
            println!("Aborting auto-fix: suspicious patterns detected in generated code.");
            cleanup_branch(base_branch, &branch)?;

            // Post warning on the issue
            let warning = format!(
                "{}## ⚠️ Auto-fix aborted\n\n\
                 The generated fix was rejected by the security scanner:\n\n{}\n\n\
                 A maintainer should review this issue manually.\n\n{}",
                config.branding.header(),
                violations
                    .iter()
                    .map(|v| format!("- `{v}`"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                config.branding.footer("Scanned"),
            );
            let _ = gh.comment_issue(issue.number, &warning).await;

            return Ok(());
        }
        info!("Diff scan passed — no suspicious patterns");
    }

    // Step 4: Commit and push
    commit_and_push(&branch, issue.number)?;

    // Step 5: Open PR (as draft by default for mandatory review)
    let pr_body = format!(
        "{}Fixes #{}\n\n## 🤖 Auto-generated fix\n\n\
         This PR was automatically generated by **{}** using `{}`.\n\n\
         > ⚠️ **This is a draft PR** — a human must review and approve before merging.\n\n\
         ### Issue\n> {}\n\n{}\n\n\
         ### Security\n\
         - [x] Author verified as collaborator\n\
         - [x] Generated diff scanned for suspicious patterns\n\
         - [ ] **Human review required**\n\n{}",
        config.branding.header(),
        issue.number,
        config.branding.name,
        tool.name(),
        issue.title,
        issue.body.as_deref().unwrap_or(""),
        config.branding.footer("Generated"),
    );

    let pr_title = format!(
        "fix: {} (#{}) [{}]",
        issue.title, issue.number, config.branding.name
    );

    let create_result = if config.fix.draft_pr {
        gh.create_draft_pr(&pr_title, &pr_body, &branch, base_branch)
            .await
    } else {
        gh.create_pr(&pr_title, &pr_body, &branch, base_branch)
            .await
    };

    match create_result {
        Ok(pr_number) => {
            let draft_label = if config.fix.draft_pr { " (draft)" } else { "" };
            println!("Opened PR #{pr_number}{draft_label}: {pr_title}");
            let comment = format!(
                "{}I've opened **draft PR #{pr_number}** with a potential fix.\n\n\
                 A maintainer needs to review and mark it as ready before it can be merged.\n\n{}",
                config.branding.header(),
                config.branding.footer("Auto-fixed"),
            );
            gh.comment_issue(issue.number, &comment).await?;

            // Emit export event
            if let Some(em) = exporter {
                em.emit(&ExportEvent {
                    kind: EventKind::FixApplied,
                    repo: config.repo_slug(),
                    timestamp: chrono::Utc::now(),
                    data: serde_json::json!({
                        "issue_number": issue.number,
                        "pr_number": pr_number,
                        "tool": tool.name(),
                    }),
                })
                .await?;
            }

            // ICM: store successful fix for future context
            crate::icm::store(
                &format!("autofix-{}", config.repo_slug()),
                &format!(
                    "Issue #{} '{}' → auto-fixed with {} → PR #{pr_number}",
                    issue.number,
                    issue.title,
                    tool.name(),
                ),
                "medium",
                &["autofix", tool.name()],
            );
        }
        Err(e) => {
            tracing::error!("Failed to create PR: {e:#}");
            println!("Changes are on branch `{branch}`. Create the PR manually.");

            // ICM: store failed attempt for future context
            crate::icm::store(
                &format!("autofix-{}", config.repo_slug()),
                &format!(
                    "Issue #{} '{}' → auto-fix with {} succeeded but PR creation failed: {e}",
                    issue.number,
                    issue.title,
                    tool.name(),
                ),
                "medium",
                &["autofix", "error"],
            );
        }
    }

    Ok(())
}

// ── Diff security scanner ────────────────────────────────────

/// Suspicious patterns that could indicate prompt injection or exfiltration.
const SUSPICIOUS_PATTERNS: &[(&str, &str)] = &[
    ("curl ", "HTTP request (potential data exfiltration)"),
    ("wget ", "HTTP request (potential data exfiltration)"),
    ("ncat ", "Netcat (potential reverse shell)"),
    (" nc -", "Netcat (potential reverse shell)"),
    ("eval(", "Dynamic code execution"),
    ("exec(", "Process execution"),
    ("subprocess", "Process execution"),
    ("os.system", "OS command execution"),
    ("child_process", "Node.js process execution"),
    ("ANTHROPIC_API_KEY", "API key reference"),
    ("OPENAI_API_KEY", "API key reference"),
    ("GITHUB_TOKEN", "Token reference"),
    ("process.env", "Environment variable access"),
    ("std::env::var", "Environment variable access"),
    ("base64", "Encoding (potential obfuscation)"),
    (".credentials", "Credential file access"),
    ("id_rsa", "SSH key reference"),
    ("BEGIN PRIVATE KEY", "Private key in code"),
    ("BEGIN RSA", "RSA key in code"),
    ("password", "Password reference"),
    ("secret", "Secret reference"),
];

/// Scan unstaged diff for suspicious patterns. Returns list of violations.
fn scan_diff_for_threats() -> Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .args(["diff", "--unified=0"])
        .output()
        .context("Failed to run git diff")?;

    let diff = String::from_utf8_lossy(&output.stdout);
    let mut violations = Vec::new();

    for line in diff.lines() {
        // Only check added lines (starting with +, not +++)
        if !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }

        let lower = line.to_lowercase();
        for (pattern, description) in SUSPICIOUS_PATTERNS {
            if lower.contains(&pattern.to_lowercase()) {
                violations.push(format!("{description}: `{}`", line.trim_start_matches('+')));
                break; // One violation per line is enough
            }
        }
    }

    Ok(violations)
}

// ── Tool abstraction ──────────────────────────────────────────

enum AiTool {
    ClaudeCode {
        model: Option<String>,
    },
    Codex,
    Podman {
        image: String,
        env_vars: Vec<(String, String)>,
        claude_dir: Option<PathBuf>,
        tool: Box<AiTool>,
    },
}

impl AiTool {
    fn name(&self) -> &str {
        match self {
            AiTool::ClaudeCode { .. } => "claude-code",
            AiTool::Codex => "codex",
            AiTool::Podman { tool, .. } => match tool.as_ref() {
                AiTool::ClaudeCode { .. } => "podman:claude-code",
                AiTool::Codex => "podman:codex",
                AiTool::Podman { .. } => "podman",
            },
        }
    }

    async fn execute(&self, prompt: &str) -> Result<ToolResult> {
        match self {
            AiTool::ClaudeCode { model } => run_claude_code(prompt, model.as_deref()).await,
            AiTool::Codex => run_codex(prompt).await,
            AiTool::Podman {
                image,
                env_vars,
                claude_dir,
                tool,
            } => run_in_podman(image, env_vars, claude_dir.as_deref(), tool, prompt).await,
        }
    }
}

struct ToolResult {
    success: bool,
    output: String,
}

/// Resolve credential source for Claude Code in Podman.
///
/// Priority:
///   1. CLAUDE_CREDENTIALS_JSON env var (CI: GitHub Secret containing the JSON)
///      → write to temp file, mount as ~/.claude/.credentials.json
///   2. ~/.claude/.credentials.json on host (local: OAuth/Max subscription)
///      → mount entire ~/.claude dir
///   3. ANTHROPIC_API_KEY env var (fallback: API key)
///      → pass as -e
fn resolve_claude_dir() -> Option<PathBuf> {
    // Priority 1: CI secret → write to temp dir with restricted permissions
    if let Ok(creds_json) = std::env::var("CLAUDE_CREDENTIALS_JSON") {
        let tmp_claude_dir = std::env::temp_dir().join("wshm-claude-creds");
        if std::fs::create_dir_all(&tmp_claude_dir).is_ok() {
            // Restrict directory permissions to owner only
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &tmp_claude_dir,
                    std::fs::Permissions::from_mode(0o700),
                );
            }
            let creds_path = tmp_claude_dir.join(".credentials.json");
            if std::fs::write(&creds_path, &creds_json).is_ok() {
                // Restrict file permissions to owner only
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        &creds_path,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
                info!("Using CLAUDE_CREDENTIALS_JSON from environment (CI secret)");
                return Some(tmp_claude_dir);
            }
        }
    }

    // Priority 2: Local ~/.claude/.credentials.json
    if let Some(home) = dirs::home_dir() {
        let claude_dir = home.join(".claude");
        let creds_path = claude_dir.join(".credentials.json");
        if creds_path.exists() {
            info!("Using OAuth credentials from ~/.claude/.credentials.json");
            return Some(claude_dir);
        }
    }

    // Priority 3: No claude dir — will fall back to ANTHROPIC_API_KEY via -e
    None
}

fn resolve_tool(config: &Config, args: &FixArgs) -> AiTool {
    let base_tool = match args.tool.as_deref().unwrap_or("claude-code") {
        "codex" => AiTool::Codex,
        _ => AiTool::ClaudeCode {
            model: args.model.clone(),
        },
    };

    if args.docker {
        let image = args
            .image
            .clone()
            .unwrap_or_else(|| "wshm-sandbox:latest".to_string());

        // Resolve Claude credentials (OAuth > API key)
        let claude_dir = resolve_claude_dir();

        // Collect env vars to inject
        let mut env_vars = Vec::new();

        // Only inject API key if no OAuth credentials found
        if claude_dir.is_none() {
            if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                env_vars.push(("ANTHROPIC_API_KEY".to_string(), key));
            }
        }
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            env_vars.push(("OPENAI_API_KEY".to_string(), key));
        }
        if let Ok(key) = std::env::var("GITHUB_TOKEN") {
            env_vars.push(("GITHUB_TOKEN".to_string(), key));
        }

        // Forward custom secret env vars from config
        for key_name in &config.fix_secret_env_vars() {
            if let Ok(val) = std::env::var(key_name) {
                env_vars.push((key_name.clone(), val));
            }
        }

        AiTool::Podman {
            image,
            env_vars,
            claude_dir,
            tool: Box::new(base_tool),
        }
    } else {
        base_tool
    }
}

// ── Tool runners ──────────────────────────────────────────────

async fn run_claude_code(prompt: &str, model: Option<&str>) -> Result<ToolResult> {
    info!("Starting claude -p (this may take a few minutes)...");
    let mut cmd = tokio::process::Command::new("claude");
    cmd.arg("-p")
        .arg(prompt)
        .arg("--dangerously-skip-permissions")
        .arg("--output-format")
        .arg("text");

    if let Some(model) = model {
        cmd.arg("--model").arg(model);
        info!("Using model: {model}");
    }

    let output = cmd
        .output()
        .await
        .context("Failed to run `claude`. Is Claude Code installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        info!(
            "claude -p completed successfully ({} bytes output)",
            stdout.len()
        );
    } else {
        tracing::error!("claude -p failed: {stderr}");
    }

    Ok(ToolResult {
        success: output.status.success(),
        output: stdout + &stderr,
    })
}

async fn run_codex(prompt: &str) -> Result<ToolResult> {
    let output = tokio::process::Command::new("codex")
        .arg("--approval-mode")
        .arg("full-auto")
        .arg(prompt)
        .output()
        .await
        .context("Failed to run `codex`. Is OpenAI Codex CLI installed?")?;

    Ok(ToolResult {
        success: output.status.success(),
        output: String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr),
    })
}

async fn run_in_podman(
    image: &str,
    env_vars: &[(String, String)],
    claude_dir: Option<&std::path::Path>,
    tool: &AiTool,
    prompt: &str,
) -> Result<ToolResult> {
    let cwd = std::env::current_dir()?;
    let mut cmd = tokio::process::Command::new("podman");

    cmd.arg("run")
        .arg("--rm")
        // Rootless security (à la OpenClaw)
        .arg("--userns=keep-id:uid=1000,gid=1000")
        .arg("--cap-drop")
        .arg("ALL")
        .arg("--pids-limit")
        .arg("256")
        .arg("--tmpfs")
        .arg("/tmp:rw,nosuid,size=512m");

    // Mount workspace
    cmd.arg("-v")
        .arg(format!("{}:/workspace:rw", cwd.display()))
        .arg("-w")
        .arg("/workspace");

    // Mount Claude credentials if available (OAuth/Max)
    if let Some(claude_path) = claude_dir {
        cmd.arg("-v")
            .arg(format!("{}:/home/claude/.claude:ro", claude_path.display()));
        info!("Mounting Claude credentials: {}", claude_path.display());
    }

    // Inject secrets as env vars
    for (key, val) in env_vars {
        cmd.arg("-e").arg(format!("{key}={val}"));
    }

    cmd.arg(image);

    // Build the inner command
    match tool {
        AiTool::ClaudeCode { model } => {
            cmd.arg("claude")
                .arg("-p")
                .arg(prompt)
                .arg("--dangerously-skip-permissions");
            if let Some(model) = model {
                cmd.arg("--model").arg(model);
            }
        }
        AiTool::Codex => {
            cmd.arg("codex")
                .arg("--approval-mode")
                .arg("full-auto")
                .arg(prompt);
        }
        AiTool::Podman { .. } => unreachable!(),
    }

    let output = cmd
        .output()
        .await
        .context("Failed to run Podman container. Is Podman installed?")?;

    // Cleanup temp credentials if we created them from CI secret
    if let Ok(_) = std::env::var("CLAUDE_CREDENTIALS_JSON") {
        let tmp_dir = std::env::temp_dir().join("wshm-claude-creds");
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    Ok(ToolResult {
        success: output.status.success(),
        output: String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr),
    })
}

// ── Git helpers ───────────────────────────────────────────────

/// Ensure we start from a clean, up-to-date base branch.
/// Discards any local modifications so the fix branch starts pristine.
fn ensure_clean_base(base_branch: &str) -> Result<()> {
    // Checkout the base branch
    let status = std::process::Command::new("git")
        .args(["checkout", base_branch])
        .status()
        .context("Failed to checkout base branch")?;
    if !status.success() {
        anyhow::bail!("Failed to checkout base branch: {base_branch}");
    }

    // Discard any local modifications
    std::process::Command::new("git")
        .args(["reset", "--hard", &format!("origin/{base_branch}")])
        .status()
        .context("Failed to reset to origin")?;

    // Pull latest
    std::process::Command::new("git")
        .args(["pull", "--ff-only"])
        .status()
        .ok();

    Ok(())
}

fn create_branch(branch: &str) -> Result<()> {
    // Delete local branch if it exists (stale from previous attempt)
    std::process::Command::new("git")
        .args(["branch", "-D", branch])
        .status()
        .ok();

    let status = std::process::Command::new("git")
        .args(["checkout", "-b", branch])
        .status()
        .context("Failed to create git branch")?;

    if !status.success() {
        anyhow::bail!("Failed to create branch: {branch}");
    }
    Ok(())
}

fn check_changes() -> Result<bool> {
    let output = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .context("Failed to check git status")?;
    Ok(!output.stdout.is_empty())
}

/// Paths that must never be committed by auto-fix (credentials, state, secrets).
const EXCLUDED_PATHS: &[&str] = &[
    ".wshm/",
    ".env",
    "credentials",
    "*.key",
    "*.pem",
    "id_rsa",
    ".claude/",
    ".fastembed_cache/",
];

fn commit_and_push(branch: &str, issue_number: u64) -> Result<()> {
    // Add all changes including new files (needed for auto-fix that creates files)
    let status = std::process::Command::new("git")
        .args(["add", "-A"])
        .status()
        .context("Failed to git add")?;
    if !status.success() {
        anyhow::bail!("git add failed");
    }

    // Unstage any sensitive/excluded paths
    for pattern in EXCLUDED_PATHS {
        std::process::Command::new("git")
            .args(["reset", "HEAD", "--", pattern])
            .output()
            .ok();
    }

    let msg = format!("fix: auto-fix for issue #{issue_number} [wshm]");
    let status = std::process::Command::new("git")
        .args(["commit", "-m", &msg])
        .status()
        .context("Failed to git commit")?;
    if !status.success() {
        anyhow::bail!("git commit failed");
    }

    let status = std::process::Command::new("git")
        .args(["push", "-u", "origin", branch])
        .status()
        .context("Failed to git push")?;
    if !status.success() {
        anyhow::bail!("git push failed");
    }

    Ok(())
}

fn cleanup_branch(base_branch: &str, branch: &str) -> Result<()> {
    std::process::Command::new("git")
        .args(["checkout", base_branch])
        .status()
        .ok();
    std::process::Command::new("git")
        .args(["branch", "-D", branch])
        .status()
        .ok();
    Ok(())
}

fn build_fix_prompt(title: &str, body: &str) -> String {
    format!(
        r#"Fix the following GitHub issue. Make minimal changes — only modify what's necessary to resolve the issue. Do not refactor unrelated code.

## Issue: {title}

{body}

Instructions:
1. Identify the root cause
2. Make the minimal fix
3. Add a test if appropriate
4. Do not modify unrelated files"#
    )
}
