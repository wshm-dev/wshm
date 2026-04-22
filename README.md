<p align="center">
  <img src="assets/wizard.png" alt="wshm — Your repo's wish is my command" width="600"/>
</p>

<h1 align="center">wshm</h1>
<p align="center"><em>Your repo's wish is my command.</em></p>

<p align="center">
  <strong>AI-powered GitHub agent for repository maintenance.</strong><br>
  Triage issues, review PRs, auto-fix bugs, manage merge queues — all from a single binary.
</p>

<p align="center">
  <a href="https://wshm.dev">Website</a> •
  <a href="#features">Features</a> •
  <a href="#install">Install</a> •
  <a href="#cli-reference">CLI</a> •
  <a href="#license">License</a>
</p>

<p align="center">
  English •
  <a href="docs/README.fr.md">Français</a> •
  <a href="docs/README.es.md">Español</a> •
  <a href="docs/README.de.md">Deutsch</a> •
  <a href="docs/README.ja.md">日本語</a> •
  <a href="docs/README.zh.md">中文</a> •
  <a href="docs/README.ko.md">한국어</a> •
  <a href="docs/README.pt.md">Português</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-SSPL--1.0-purple.svg" alt="License: SSPL v1"/></a>
  <a href="https://wshm.dev"><img src="https://img.shields.io/badge/website-wshm.dev-blue.svg" alt="Website"/></a>
</p>

---

> **Source-Available Software** — This project is licensed under the [Server Side Public License (SSPL v1)](LICENSE), the same license used by MongoDB and Elastic. You can freely use, study, and modify the code. You cannot offer it as a competing managed service. See [License](#license) for details.

## Features

- **Issue Triage** — Automatically classify, label, and prioritize new issues using AI
- **PR Analysis** — Summarize PRs, assess risk, generate review checklists
- **Auto-Fix** — Generate and open draft PRs for simple bugs (confidence-gated)
- **Merge Queue** — Score and rank PRs by readiness, auto-merge when above threshold
- **Conflict Resolution** — Detect and auto-resolve merge conflicts (never force-pushes)
- **Inline Review** — AI-powered line-by-line code review comments
- **Auto-Assign** — Weighted random assignment of maintainers to issues and PRs
- **Labels Blacklist** — Prevent specific labels from ever being applied
- **Periodic Retriage** — Re-evaluate stale triage results on a schedule
- **Notifications** — Daily roadmap to Discord, Slack, Teams, or webhook
- **Dashboard & Reports** — HTML dashboards and markdown/PDF reports
- **Fully Customizable** — Templates for every comment, branding, and behavior

## How it works

```
              ┌─────────────┐
              │  GitHub API  │
              └──────┬───────┘
                     │ sync (ETag + incremental)
                     ▼
              ┌─────────────┐
              │  SQLite DB   │  ← .wshm/state.db (committed to repo)
              └──────┬───────┘
                     │ read (instant, no network)
                     ▼
              ┌─────────────┐
              │  AI Engine   │  ← Your API keys (Anthropic, OpenAI, Google, etc.)
              └──────┬───────┘
                     │ classify / analyze / fix
                     ▼
              ┌─────────────┐
              │   Actions    │  ← Label, comment, open PR, merge, assign
              └─────────────┘
```

**Zero infrastructure.** One binary. Your keys. Your data. Runs as CLI, GitHub Action, or persistent daemon.

## Install

### Homebrew (macOS / Linux)

```bash
brew tap wshm-dev/tap
brew install wshm
```

### Cargo

```bash
cargo install wshm
```

### Build from source

```bash
git clone https://github.com/wshm-dev/wshm.git
cd wshm
cargo build --release
```

### Prebuilt binaries

Download from [GitHub Releases](https://github.com/wshm-dev/wshm/releases).

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

## Pipelines

### Pipeline 1 — Issue Triage
```
New Issue → AI Classification → Label + Priority + Comment
                                  ├── duplicate? → close with link
                                  ├── needs-info? → ask for details
                                  ├── simple bug? → auto-fix (draft PR)
                                  └── feature? → label + backlog
```

### Pipeline 2 — PR Analysis
```
New PR → Fetch diff + CI status → AI Analysis → Summary + Risk + Checklist
                                                  └── Auto-label + comment
```

### Pipeline 3 — Merge Queue
```
Open PRs → Score (CI, reviews, age, risk, conflicts) → Ranked list
                                                         └── Auto-merge if above threshold
```

### Pipeline 4 — Conflict Resolution
```
Open PRs → Check mergeable → Conflicting? → Rebase from main
                                              └── AI resolution (new commit, never force-push)
```

## Daemon Mode (H24)

wshm can run as a persistent daemon that reacts to GitHub events in real-time.

### Webhook Mode (recommended)

```bash
wshm daemon --apply --secret "whsec_your_webhook_secret"
```

Then configure a GitHub webhook on your repo:
- **URL:** `http://your-server:3000/webhook`
- **Content type:** `application/json`
- **Secret:** same as `--secret`
- **Events:** Issues, Pull requests, Issue comments

### Polling Mode (no public IP needed)

```bash
wshm daemon --apply --poll --no-server
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

| Provider | Env Var | Local |
|----------|---------|-------|
| `anthropic` (default) | `ANTHROPIC_API_KEY` | — |
| `openai` | `OPENAI_API_KEY` | — |
| `google` | `GOOGLE_API_KEY` | — |
| `mistral` | `MISTRAL_API_KEY` | — |
| `groq` | `GROQ_API_KEY` | — |
| `deepseek` | `DEEPSEEK_API_KEY` | — |
| `xai` | `XAI_API_KEY` | — |
| `ollama` | — | yes |
| `local` | — | yes (phi4-mini, smollm3-3b, qwen3-4b) |

### Claude Subscription (no API key needed)

```bash
wshm login --claude    # Uses your existing Claude Max/Pro/Team subscription
```

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
| `wshm notify` | Send priority summary (Discord/Slack/Teams/webhook) |
| `wshm daemon` | Start persistent daemon (webhook + polling) |
| `wshm backup` | Backup the local SQLite database |
| `wshm backup --output <path>` | Backup to a specific file |
| `wshm restore <file>` | Restore from a backup file |
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

## Configuration

Everything is configured in `.wshm/config.toml`:

```toml
[ai]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[triage]
enabled = true
auto_fix = false
auto_fix_confidence = 0.85
retriage_interval_hours = 24

[pr]
enabled = true
auto_label = true
risk_labels = true

[queue]
enabled = true
merge_threshold = 15
strategy = "rebase"              # merge, rebase, squash

[assign]
enabled = true

[[assign.issues]]
user = "alice"
weight = 70

[[assign.prs]]
user = "bob"
weight = 50

labels_blacklist = ["do-not-touch", "manual-only"]

[branding]
name = "my-bot"
url = "https://my-project.dev"
command_prefix = "/my-bot"

[notify]
on_run = true

[[notify.discord]]
url = "https://discord.com/api/webhooks/ID/TOKEN"

[[notify.slack]]
url = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"

[[notify.teams]]
url = "https://outlook.office.com/webhook/YOUR/WEBHOOK/URL"
```

## Auto-Fix with Podman Sandbox

```bash
# Build the sandbox image
podman build -f Dockerfile.sandbox -t wshm-sandbox:latest .

# Run Claude Code in Podman sandbox
wshm fix --issue 42 --docker --apply

# Use Codex instead
wshm fix --issue 42 --docker --tool codex --apply
```

## Notifications

```bash
wshm notify          # send summary now
wshm run --apply     # full cycle + auto-notify (if on_run = true)
```

The summary includes: open issues, untriaged count, high-priority issues, open PRs, high-risk PRs, and merge conflicts. Formatted natively for each platform (Discord embeds, Slack blocks, Teams adaptive cards).

## Reports

```bash
wshm report --format md
wshm report --format html --output report.html
wshm report --format pdf --output report.pdf
```

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
      - uses: wshm-dev/wshm@main
        with:
          github-token: ${{ secrets.GITHUB_TOKEN }}
          ai-api-key: ${{ secrets.ANTHROPIC_API_KEY }}
```

## Deploy on a VM

```bash
cargo install wshm
wshm login
wshm config init
wshm daemon --apply --poll
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

## Backup & Restore

wshm stores all its state in `.wshm/state.db` (SQLite). Back it up before upgrades or migrations.

```bash
# Backup (atomic via VACUUM INTO, includes WAL checkpoint)
wshm backup                        # → ~/.wshm/backups/state-20260422T143000.db
wshm backup --output my-backup.db  # custom path

# Restore
wshm restore ~/.wshm/backups/state-20260422T143000.db
# The existing DB is automatically saved as state.db.pre-restore before overwrite
```

**From the TUI:** press `b` to backup instantly, `B` to restore (prompts for file path).

**From the web API (daemon mode):**

| Method | Endpoint | Action |
|--------|----------|--------|
| `GET` | `/api/v1/backups` | List available backup files |
| `POST` | `/api/v1/backup` | Create a new backup |
| `POST` | `/api/v1/restore` | Restore from a backup (`{"path": "..."}`) |

## Safety

- **Dry-run by default** — `--apply` required to perform actions
- **Confidence gates** — never acts autonomously below threshold (default 0.85)
- **Never force-pushes** — conflict resolution uses new commits
- **Idempotent** — re-running = same result, no duplicate comments
- **Token security** — always from env vars, never in config files
- **Transparent** — every action posts a comment explaining what and why

## Architecture

See [CLAUDE.md](CLAUDE.md) for the full architecture document.

## License

This project is licensed under the **[Server Side Public License (SSPL v1)](LICENSE)**.

### What you CAN do

- Use wshm for any purpose (personal, commercial, enterprise)
- Read, study, and audit every line of code
- Modify the code for your own internal use
- Contribute back to the project

### What you CANNOT do

- Offer wshm (or a modified version) as a managed/hosted service to third parties without releasing your entire service stack under the SSPL
- Build a competing commercial service based on this code

### Why SSPL?

We believe in transparency. You should be able to audit the tool that manages your repositories. But we also need to sustain development — the SSPL ensures that no one can take this work and sell it as their own service without contributing back.

This is the same model used by **MongoDB**, **Elastic**, and **Graylog**.

For enterprise licensing or questions: [contact@wshm.dev](mailto:contact@wshm.dev)

## Disclaimer

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR
ANY CLAIM, DAMAGES OR OTHER LIABILITY ARISING FROM THE USE OF THIS SOFTWARE.

**Precompiled binaries**: Official precompiled binaries are provided for
convenience via [GitHub Releases](https://github.com/wshm-dev/wshm/releases)
and [Homebrew](https://github.com/wshm-dev/homebrew-tap). These binaries are
built from the exact source code available in this repository. However, they
are distributed **as-is with no warranty**. If you require full
reproducibility or have security concerns, you are encouraged to build from
source using the instructions above. By downloading and using a precompiled
binary, you acknowledge that you do so at your own risk.

**AI-generated actions**: wshm uses AI models to classify issues, analyze
pull requests, and suggest code fixes. All automated actions are **dry-run by
default** and require explicit `--apply` to take effect. AI outputs may
contain errors — always review before applying. wshm-dev is not responsible
for any actions taken by the AI on your repositories.

---

<p align="center">
  <sub>Built with Rust. Zero infra. One binary.</sub><br>
  <sub>&copy; 2026 <a href="https://wshm.dev">wshm-dev</a></sub>
</p>
