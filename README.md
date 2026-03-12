[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

# wshm (wishmaster)

> Your repo's wish is my command.

AI-powered GitHub agent for OSS maintainers. Triage issues, auto-fix simple bugs, analyze PRs, resolve conflicts, generate reports. Built in Rust. Zero infra. One binary.

## Install

```bash
cargo install wshm
```

Or build from source:

```bash
git clone https://github.com/pszymkowiak/wshm.git
cd wshm
cargo build --release
```

## Quick Start

```bash
# 1. Login (GitHub + AI provider)
wshm login

# 2. Initialize config
wshm config init

# 3. Sync & triage (dry-run)
wshm sync
wshm triage

# 4. Apply actions
wshm triage --apply
```

## Daemon Mode (H24)

wshm can run as a persistent daemon that reacts to GitHub events in real-time.

### Webhook Mode (recommended)

Requires a public URL (IP, domain, or tunnel like ngrok/cloudflare).

```bash
# Setup
wshm login
wshm config init

# Start daemon
wshm daemon --apply --secret "whsec_your_webhook_secret"
```

Then configure a GitHub webhook on your repo:
- **URL:** `http://your-server:3000/webhook`
- **Content type:** `application/json`
- **Secret:** same as `--secret`
- **Events:** Issues, Pull requests, Issue comments

### Polling Mode (no public IP needed)

Uses the GitHub Events API — no webhook configuration required.

```bash
# Start daemon with polling (no HTTP server)
wshm daemon --apply --poll --no-server

# Or hybrid: webhook + polling fallback
wshm daemon --apply --poll
```

| Flag | Description |
|------|-------------|
| `--apply` | Perform actions (dry-run by default) |
| `--poll` | Enable GitHub Events API polling |
| `--poll-interval <N>` | Polling interval in seconds (default: 30) |
| `--no-server` | Disable the HTTP webhook server |
| `--bind <addr>` | Bind address (default: `0.0.0.0:3000`) |
| `--secret <s>` | Webhook HMAC secret |

### What the Daemon Does

| Trigger | Action |
|---------|--------|
| New issue opened | Auto-triage (classify, label, comment) |
| New/updated PR | AI analysis (type, risk, review checklist) |
| `/wshm triage` comment | Re-triage the issue |
| `/wshm analyze` comment | Re-analyze the PR |
| `/wshm review` comment | Post inline AI code review |
| `/wshm label <name>` | Add a label |
| `/wshm unlabel <name>` | Remove a label |
| `/wshm queue` | Show merge queue position |
| `/wshm health` | PR health check |
| `/wshm help` | List available commands |
| Every 5 min (scheduler) | Incremental sync + triage untriaged issues |

### Health Check

```bash
curl http://localhost:3000/health
# {"status":"ok","apply":true,"pending_events":0,"repo":"owner/repo"}
```

## Authentication

### Interactive Login

```bash
wshm login              # Setup GitHub + AI provider
wshm login --github     # GitHub only (uses gh auth login)
wshm login --ai         # AI provider only (prompts for API key)
wshm login --status     # Show current auth status
```

Credentials are stored in `.wshm/credentials` (chmod 600, gitignored).

### Token Priority

| Priority | GitHub | AI |
|----------|--------|----|
| 1 | `GITHUB_TOKEN` / `WSHM_TOKEN` env var | `ANTHROPIC_API_KEY` env var |
| 2 | `gh auth token` (gh CLI) | — |
| 3 | `.wshm/credentials` (from `wshm login`) | `.wshm/credentials` |

### Supported AI Providers

| Provider | Env Var | Models |
|----------|---------|--------|
| `anthropic` (default) | `ANTHROPIC_API_KEY` | claude-sonnet-4-20250514, etc. |
| `openai` | `OPENAI_API_KEY` | gpt-4o, etc. |
| `google` | `GOOGLE_API_KEY` | gemini-2.5-pro, etc. |
| `mistral` | `MISTRAL_API_KEY` | mistral-large, etc. |
| `groq` | `GROQ_API_KEY` | llama-3, etc. |
| `deepseek` | `DEEPSEEK_API_KEY` | deepseek-chat, etc. |
| `xai` | `XAI_API_KEY` | grok-3, etc. |
| `ollama` | — (local) | Any Ollama model |
| `local` | — | phi4-mini, smollm3-3b, qwen3-4b, etc. |

## CLI Reference

| Command | Description |
|---------|-------------|
| `wshm` | Show status from cache (instant) |
| `wshm sync` | Force full sync from GitHub |
| `wshm login` | Authenticate with GitHub + AI provider |
| `wshm triage [--issue <N>]` | Classify issues (or single issue) |
| `wshm pr [--pr <N>]` | Analyze PRs (or single PR) |
| `wshm queue` | Show ranked merge queue |
| `wshm conflicts` | Detect conflicting PRs |
| `wshm run` | Full cycle: triage + analyze + queue + conflicts |
| `wshm review [--pr <N>]` | Inline AI code review on PR diffs |
| `wshm health` | PR health: duplicates, stale/zombie PRs |
| `wshm fix --issue <N>` | Auto-generate a fix PR from an issue |
| `wshm report` | Generate report (md/html/pdf) |
| `wshm changelog` | Generate changelog from merged PRs |
| `wshm dashboard` | Generate metrics dashboard (HTML + charts) |
| `wshm daemon` | Start persistent daemon (webhook + polling) |
| `wshm config init` | Create `.wshm/config.toml` template |
| `wshm model list` | List available local AI models |
| `wshm model pull <name>` | Download a model for local inference |

### Global Flags

| Flag | Description |
|------|-------------|
| `--apply` | Perform actions (dry-run by default) |
| `--offline` | Skip GitHub sync, use cached data only |
| `--verbose` / `-v` | Detailed output |
| `--json` | JSON output for scripting |
| `--repo <owner/repo>` | Override detected repo |

## Branding

Customize the bot's identity for your organization.

```toml
# .wshm/config.toml
[branding]
name = "jarvis"                                # Bot name in comments
url = "https://acme.com/jarvis"                # Link in footers
avatar_url = "https://acme.com/logo.png"       # Logo in comment headers
tagline = "Your AI repo assistant"             # Subtitle in headers
command_prefix = "/jarvis"                     # Slash command prefix
footer_template = "*{action} by [{name}]({url})*"  # Custom footer
```

**Before (default):**
> *Triaged by [wshm](https://github.com/pszymkowiak/wshm)*

**After (custom branding):**
> <img src="https://acme.com/logo.png" width="20" height="20"> **jarvis** — Your AI repo assistant
>
> ## 🔍 Triage Summary
> ...
>
> *Triaged by [jarvis](https://acme.com/jarvis)*

All GitHub comments, slash commands, PR titles, and footers adapt automatically.

## Auto-Fix with Podman Sandbox

wshm can auto-generate fix PRs from issues using Claude Code or Codex, optionally inside a rootless Podman container.

```bash
# Build the sandbox image
podman build -f Dockerfile.sandbox -t wshm-sandbox:latest .

# Dry-run (shows what would happen)
wshm fix --issue 42 --docker

# Run Claude Code in Podman sandbox
wshm fix --issue 42 --docker --apply

# Use Codex instead
wshm fix --issue 42 --docker --tool codex --apply
```

### Credential Injection (Priority Order)

| Priority | Source | How | Context |
|----------|--------|-----|---------|
| 1 | `CLAUDE_CREDENTIALS_JSON` | GitHub Secret -> temp file -> volume mount | CI |
| 2 | `~/.claude/.credentials.json` | Volume mount (`-v ~/.claude:....:ro`) | Local (Max/Pro) |
| 3 | `ANTHROPIC_API_KEY` | Env var (`-e`) | Fallback API key |

**Security** (rootless Podman, like OpenClaw):
- `--userns=keep-id` (no root)
- `--cap-drop ALL`
- `--pids-limit 256`
- Credentials mounted read-only

## Reports

```bash
# Markdown report
wshm report --format md

# HTML report (SLA metrics, PR health, duplicates)
wshm report --format html --output report.html

# PDF report
wshm report --format pdf --output report.pdf

# Full cycle then report
wshm run --apply
wshm report --format html
```

Reports include: issue triage, PR analysis, merge queue ranking, SLA tracking, PR health (duplicates, stale/zombie).

## GitHub Action

```yaml
name: wshm
on:
  issues:
    types: [opened]
  pull_request:
    types: [opened, synchronize]
  issue_comment:
    types: [created]
  schedule:
    - cron: '0 */6 * * *'

jobs:
  wshm:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: pszymkowiak/wshm@main
        with:
          github-token: ${{ secrets.GITHUB_TOKEN }}
          ai-api-key: ${{ secrets.ANTHROPIC_API_KEY }}
          claude-credentials-json: ${{ secrets.CLAUDE_CREDENTIALS_JSON }}
```

## Deploy on a VM

```bash
# 1. Clone your repo
git clone git@github.com:owner/repo.git && cd repo

# 2. Install wshm
cargo install wshm

# 3. Login
wshm login

# 4. Init config
wshm config init

# 5. Start daemon (runs 24/7)
wshm daemon --apply --poll

# Or with webhook + systemd
wshm daemon --apply --secret "$WSHM_WEBHOOK_SECRET"
```

### Systemd Service

```ini
# /etc/systemd/system/wshm.service
[Unit]
Description=wshm daemon
After=network.target

[Service]
Type=simple
User=deploy
WorkingDirectory=/home/deploy/repo
ExecStart=/home/deploy/.cargo/bin/wshm daemon --apply --poll
Restart=always
RestartSec=10
Environment=RUST_LOG=info

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now wshm
journalctl -u wshm -f
```

## Architecture

See [CLAUDE.md](CLAUDE.md) for the full architecture document, including:

- SQLite cache strategy and sync rules
- The 4 pipelines (triage, PR analysis, merge queue, conflict resolution)
- Config reference
- Project structure
- AI integration patterns
- Safety principles

## License

[MIT](LICENSE)
