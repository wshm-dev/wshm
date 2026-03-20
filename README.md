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
  <a href="#features">Features</a> •
  <a href="#how-it-works">How it works</a> •
  <a href="#pipelines">Pipelines</a> •
  <a href="#configuration">Configuration</a> •
  <a href="#early-access">Early Access</a>
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

---

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

## Configuration

Everything is configured in `.wshm/config.toml`:

```toml
[ai]
provider = "anthropic"              # anthropic, openai, google, ollama, +10 more
model = "claude-sonnet-4-20250514"

[triage]
enabled = true
auto_fix = false
auto_fix_confidence = 0.85
retriage_interval_hours = 24        # re-evaluate every 24h

[pr]
enabled = true
auto_label = true
risk_labels = true

[queue]
enabled = true
merge_threshold = 15
strategy = "rebase"                 # merge, rebase, squash

[assign]
enabled = true

[[assign.issues]]
user = "alice"
weight = 70

[[assign.issues]]
user = "bob"
weight = 30

[[assign.prs]]
user = "alice"
weight = 50

[[assign.prs]]
user = "bob"
weight = 50

# Labels wshm must never apply
labels_blacklist = ["do-not-touch", "manual-only"]

[branding]
name = "my-bot"
url = "https://my-project.dev"
# triage_template = "..."          # Full custom markdown/HTML
# pr_template = "..."              # Full custom markdown/HTML
```

## Supported AI Providers

wshm works with **any AI provider** — bring your own API key or use your existing Claude subscription.

### Claude subscription (no API key needed)

| Plan | Auth | Command |
|------|------|---------|
| **Claude Max** | OAuth | `wshm login --claude` |
| **Claude Pro** | OAuth | `wshm login --claude` |
| **Claude Team** | OAuth | `wshm login --claude` |

Uses your existing Anthropic subscription directly. No API key, no extra cost, no token billing. wshm authenticates via the Claude CLI OAuth flow and uses your subscription's included usage.

### API providers

| Provider | Env Variable | Local |
|----------|-------------|-------|
| Anthropic API | `ANTHROPIC_API_KEY` | — |
| OpenAI | `OPENAI_API_KEY` | — |
| Google | `GOOGLE_API_KEY` | — |
| Mistral | `MISTRAL_API_KEY` | — |
| Groq | `GROQ_API_KEY` | — |
| DeepSeek | `DEEPSEEK_API_KEY` | — |
| xAI | `XAI_API_KEY` | — |
| Together | `TOGETHER_API_KEY` | — |
| Fireworks | `FIREWORKS_API_KEY` | — |
| Perplexity | `PERPLEXITY_API_KEY` | — |
| Cohere | `COHERE_API_KEY` | — |
| OpenRouter | `OPENROUTER_API_KEY` | — |
| Azure OpenAI | `AZURE_OPENAI_API_KEY` | — |
| Ollama | — | yes |

Priority: Claude OAuth (Max/Pro/Team) > API key > claude CLI.

## CLI

```
wshm                           # show status (from cache, instant)
wshm sync                      # force full sync from GitHub
wshm triage [--apply]          # classify open issues
wshm triage --retriage         # re-evaluate stale triage results
wshm pr analyze [--apply]      # analyze open PRs
wshm queue [--apply]           # show/execute merge queue
wshm conflicts scan [--apply]  # detect and resolve conflicts
wshm review [--apply]          # inline code review on PR diffs
wshm fix --issue <N> [--apply] # auto-generate fix from issue
wshm run [--apply]             # full cycle
wshm dashboard                 # generate HTML dashboard
wshm report                    # generate report (md/html/pdf)
wshm daemon                    # persistent daemon with webhooks/polling
```

## Modes

| Mode | Use case |
|------|----------|
| **CLI** | One-off commands, CI scripts |
| **GitHub Action** | Triggered on events (issue opened, PR created) |
| **Daemon** | Persistent process with webhook server or polling |

## Safety

- **Dry-run by default** — `--apply` required to perform actions
- **Confidence gates** — never acts autonomously below threshold (default 0.85)
- **Never force-pushes** — conflict resolution uses new commits
- **Idempotent** — re-running = same result, no duplicate comments
- **Token security** — always from env vars, never in config files
- **Transparent** — every action posts a comment explaining what and why

---

## Early Access

> **wshm is currently in private beta.**
>
> If you're interested in trying wshm on your repositories, reach out:
>
> **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)**
>
> We're looking for early adopters to shape the product. Open-source maintainers and small teams welcome.

---

<p align="center">
  <sub>Built with Rust. Zero infra. One binary.</sub><br>
  <sub>&copy; 2026 <a href="https://rtk-ai.app">rtk-ai</a></sub>
</p>
