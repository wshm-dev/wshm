<div align="center">

<img src="docs/assets/wizard.png" alt="wshm" width="180" />

# wshm

### Your repo's wish is my command.

**Five tools in one binary.** Replace CodeRabbit, Graphite, Sweep AI, Mergify, and manual triage with a single self-hosted Rust binary.

Works with GitHub, GitLab, Gitea, and Azure DevOps. Plug in Claude Max, OpenAI, or a local Ollama — your call. Your AI keys. Your data. No SaaS dependency.

[![License: SSPL](https://img.shields.io/badge/License-SSPL--1.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)
![AI providers](https://img.shields.io/badge/AI%20providers-14-purple)
![Git providers](https://img.shields.io/badge/Git%20providers-4-green)
![Self-hosted](https://img.shields.io/badge/self--hosted-yes-success)

</div>

---

<p align="center">
  <img src="docs/assets/hero.png" alt="wshm in action" width="85%" />
</p>

## Why wshm?

You shipped the code. Now you spend your Mondays triaging a hundred stale issues, hunting duplicate bug reports, babysitting the merge queue, and writing the same "thanks for the report, can you share a reproduction?" reply for the tenth time.

wshm does the boring parts so you can stay in flow.

### Replace five tools with one

| Need                  | Typical tool  | Typical price     | wshm              |
|-----------------------|---------------|-------------------|-------------------|
| AI code review        | CodeRabbit    | $24 / dev / mo    | included          |
| Merge queue           | Graphite      | $40 / user / mo   | included          |
| Issue auto-fix        | Sweep AI      | $120–240 / mo     | included (Pro)    |
| Workflow automation   | Mergify       | $15–30 / user / mo| included          |
| Issue triage          | Manual        | Engineer hours    | automated         |

A 5-dev team pays **$870+/mo** for equivalent coverage across those tools. wshm is one binary, self-hosted, your AI keys.

### Pain → gain

| Pain | wshm |
|------|------|
| Stale backlog, no one labels issues | **AI triage** classifies + labels new issues in seconds, with confidence scores |
| "Is this PR safe to merge?" review paralysis | **PR risk analysis** surfaces low/medium/high risk + a review checklist |
| Merge queue is a shared Notion doc | **Scored merge queue** auto-promotes green PRs above your threshold |
| Duplicates and zombie PRs pile up | **PR health** flags duplicates, stale, and zombie PRs automatically |
| Secrets leak into `.env` and config | **Key vault** integration (Vault, AWS, **Azure Key Vault**, GCP) |
| SaaS tools hold your data hostage | **Self-hosted**, single Rust binary, SQLite or Postgres — your data stays yours |
| OpenAI subscription fatigue | **Claude Max / Pro / Team** OAuth login — reuse your existing plan |

## See it in action

```text
$ wshm triage --apply
Syncing issues... 247 issues loaded
Classifying with claude-sonnet-4-6 (OAuth: Claude Max)...

  #412  "App crashes on M1 when opening settings"
        → type: bug, priority: high, confidence: 0.94
        → labels applied: bug, platform:macos, priority:high

  #413  "Would love dark mode support"
        → type: feature, priority: medium, confidence: 0.87
        → labels applied: enhancement, ui

  #414  "how do i install this???"
        → type: question, priority: low, confidence: 0.91
        → labels applied: question, needs-docs
        → auto-reply posted: link to docs/getting-started.md

Triage complete: 23 issues labeled, 4 auto-replied, 1 flagged as duplicate of #398
```

Merge queue view (TUI):

```text
$ wshm queue
Rank  PR    Title                             Risk   Score  CI   Reviews  Action
 1    #891  fix: race in token refresh        low    94     OK   2/2      AUTO-MERGE
 2    #885  feat: add --json flag to queue    low    88     OK   2/2      READY
 3    #879  refactor: split notify module     med    71     OK   1/2      WAIT
 4    #902  chore: bump tokio                 low    66     OK   0/2      WAIT
 5    #876  feat: webhook retries             high   42     FAIL 0/2      BLOCKED
```

## Try it in 30 seconds

```bash
brew tap wshm-dev/tap && brew install wshm
cd your-repo
wshm config init
export GITHUB_TOKEN=ghp_xxxxx
wshm login              # pick Claude Max, API key, or any of 14 providers
wshm sync
wshm triage             # dry-run — no --apply yet, just see what it would do
```

---

## Bring your own AI

wshm never ships an AI provider you have to pay extra for. Use whatever you already have.

- **Claude Max / Pro / Team** — OAuth login via the `claude` CLI, no API key, no separate billing
- **Any API provider** — Anthropic, OpenAI, Google, Mistral, Groq, DeepSeek, xAI, OpenRouter, Azure, Together, Fireworks, Cohere, Perplexity
- **Local models** — Ollama or llama.cpp, zero cost, zero data leaves your machine

```text
  Anthropic   OpenAI   Google   Mistral   Groq    DeepSeek   xAI
  OpenRouter  Ollama   Azure    Together  Fireworks Cohere   Perplexity
```

## Deploy anywhere

One static binary. No SaaS dependency. Runs where your code lives.

| Platform       | How                                                              |
|----------------|------------------------------------------------------------------|
| **VM / VPS**   | systemd service, auto-update via Homebrew, any Linux             |
| **Docker**     | `docker run ghcr.io/wshm-dev/wshm:latest daemon`, rootless Podman OK |
| **Kubernetes** | Single-pod deployment, ConfigMap for `config.toml`               |
| **GitHub Action** | No server needed, runs on every `issues` / `pull_request` event |
| **Local dev**  | `wshm tui` / `wshm triage` — works offline with Ollama           |

Forge support: **GitHub** (personal + org), **GitLab** (.com + self-managed), **Gitea / Forgejo / Codeberg**, **Azure DevOps** (Services + Server).

---

## Origin story

wshm was born out of a real need: triaging the growing backlog of issues and pull requests on [**rtk-ai/rtk**](https://github.com/rtk-ai/rtk) (Rust Token Killer). What started as a weekend script to auto-classify GitHub issues grew into a full repo agent — the tool we wished existed, now battle-tested on rtk itself before shipping to other repos.

---

## Features

### Free (this repo)

- **Issue triage** — AI classification (bug/feature/question/etc.), priority, confidence scoring, auto-labeling
- **PR analysis** — risk level (low/medium/high), type (feature/fix/docs), summary, review checklist
- **Merge queue** — scoring, auto-merge above threshold, conflict detection
- **PR health** — duplicate detection, stale/zombie PR flagging
- **Changelog** — auto-generate from merged PRs (markdown, JSON)
- **Dashboard** — HTML metrics dashboard with Chart.js graphs
- **Backup / Restore** — snapshot your `.wshm/` data for safe migration
- **Revert** — undo all wshm actions (remove comments, labels, clear analyses)
- **Notifications** — Discord, Slack, Teams, generic webhooks with HMAC signing
- **Daemon** — background polling with embedded web dashboard
- **TUI** — rich interactive terminal UI (issues, PRs, queue, stats)
- **Web service** — embedded Svelte/Vite dashboard served by the daemon
- **Multi-repo** — manage multiple repos from one daemon instance
- **14 AI providers** — Anthropic, OpenAI, Google, Mistral, Groq, DeepSeek, xAI, Ollama (local), llama.cpp, and more
- **Claude Max / Pro / Team subscription** — OAuth login via your existing Claude plan (no separate API key billing needed)
- **4 Git providers** — GitHub, GitLab, Gitea, Azure DevOps (self-hosted friendly)
- **Storage** — **SQLite** by default (zero setup, bundled), or **PostgreSQL** as an alternative primary backend
- **OIDC SSO** — login via Google, GitHub, GitLab, Azure AD

### Pro ([wshm.dev/pro](https://wshm.dev/pro))

Token-heavy AI features and enterprise integrations:

- **Inline code review** — line-by-line AI review on PR diffs
- **Auto-fix** — generate PRs from issue descriptions (Claude Code / Codex / containerized)
- **AI conflict resolution** — automatic rebase with AI assistance
- **Improvement proposals** — analyze codebase, create refactor/testing/perf issues
- **HTML/PDF reports** — full repo health reports with SLA metrics
- **Cloud exports** — ship issues/PRs/analyses to external sinks:
  - Object storage: **S3**, **Azure Blob**, **GCS**
  - Databases: **PostgreSQL**, **MySQL**, **MongoDB**, **Elasticsearch**, **OpenSearch**
  - (distinct from the primary SQLite/PostgreSQL backend above — these are *additional* destinations)
- **Secret management / Key vaults** — HashiCorp Vault, AWS Secrets Manager, **Azure Key Vault**, GCP Secret Manager (keep license keys, webhook secrets, DB URIs, API keys out of config)
- **SAML SSO** — Okta, Azure AD, custom IdP
- **RBAC + Audit log** — organizations, roles, action audit trail
- **Daemon webhook mode** — real-time GitHub webhook processing

---

## Interfaces

wshm ships four ways to interact with it, all powered by the same core.

### CLI

```bash
wshm sync               # fetch issues + PRs
wshm triage --apply     # classify and label open issues
wshm pr analyze --apply # analyze and label open PRs
wshm queue              # show ranked merge queue
wshm run --apply        # full cycle
```

### TUI

Rich interactive terminal UI — browse issues, PRs, merge queue, and stats without leaving the terminal.

```bash
wshm tui
```

<p align="center">
  <img src="docs/assets/wshm-tui.png"      alt="wshm TUI — issues"  width="32%" />
  <img src="docs/assets/wshm-tui-prs.png"  alt="wshm TUI — PRs"     width="32%" />
  <img src="docs/assets/wshm-tui-stats.png" alt="wshm TUI — stats"  width="32%" />
</p>

### Web service (daemon)

The daemon runs background polling *and* serves an embedded Svelte web dashboard on `https://127.0.0.1:3000`.

```bash
wshm daemon
```

<p align="center">
  <img src="docs/assets/wshm-web-dashboard.png" alt="Dashboard — repo status overview" width="85%" />
</p>

<p align="center">
  <img src="docs/assets/wshm-web-issues.png"  alt="Issues view"   width="48%" />
  <img src="docs/assets/wshm-web-triage.png"  alt="Triage results" width="48%" />
</p>

<p align="center">
  <img src="docs/assets/wshm-web-queue.png"   alt="Merge queue"   width="85%" />
</p>

### Daemon (headless)

Long-running process for servers — multi-repo polling, scheduled triage, notifications, Pro webhook mode.

```bash
wshm daemon --config /etc/wshm/global.toml --poll
```

---

## Quick Start

### Install

#### Homebrew (macOS + Linux)

```bash
brew tap wshm-dev/tap
brew install wshm
```

#### Shell installer (macOS + Linux)

```bash
curl -fsSL https://wshm.dev/install.sh | sh
```

#### Cargo (any platform with a Rust toolchain)

```bash
cargo install wshm-core --bin wshm
```

#### Docker

```bash
docker pull ghcr.io/wshm-dev/wshm:latest
```

#### Windows

Download the latest `wshm-x86_64-pc-windows-msvc.zip` from [GitHub Releases](https://github.com/wshm-dev/wshm/releases).

#### Supported platforms

| OS      | Architectures       | Install methods            |
|---------|---------------------|----------------------------|
| macOS   | x86_64, aarch64     | Homebrew, shell, Cargo     |
| Linux   | x86_64, aarch64 (GNU) | Homebrew, shell, Cargo, Docker |
| Windows | x86_64              | GitHub Releases, Cargo     |

### Configure

```bash
cd your-repo
wshm config init        # creates .wshm/config.toml
export GITHUB_TOKEN=ghp_xxxxx
wshm login              # interactive auth setup
```

`wshm login` supports three auth paths for Anthropic:

1. **Claude Max / Pro / Team subscription** — OAuth login (reuses `~/.claude/.credentials.json` from the `claude` CLI; no extra billing)
2. **Anthropic API key** — `export ANTHROPIC_API_KEY=sk-ant-xxxxx`
3. **Any of the 13 other providers** — OpenAI, Google, Mistral, Groq, DeepSeek, xAI, Ollama (local), llama.cpp, …

### Run

```bash
wshm sync               # fetch issues + PRs from GitHub
wshm triage --apply     # classify and label open issues
wshm pr analyze --apply # analyze and label open PRs
wshm queue              # show ranked merge queue
wshm run --apply        # full cycle: sync → triage → analyze → queue
```

---

## Secret management / Key vaults (Pro)

Any config value can be sourced from a secret vault using a `vault()` placeholder — license keys, webhook secrets, database URIs, API keys stay out of config files.

```toml
[license]
key = "vault(secret/wshm/license-key)"

[[export.webhooks]]
secret = "vault(secret/wshm/webhook-hmac)"

[export.database]
uri = "vault(secret/wshm/elastic-uri)"
```

Supported providers: **HashiCorp Vault**, **AWS Secrets Manager**, **Azure Key Vault**, **GCP Secret Manager**.
Full guide: [docs/vault.md](docs/vault.md).

---

## Documentation

Full guides in [`docs/`](./docs):

- [Getting Started](docs/getting-started.md)
- [Configuration](docs/configuration.md)
- [CLI Reference](docs/cli-reference.md)
- [Daemon & Web UI](docs/daemon.md)
- [License & Activation](docs/license.md)
- [Vault / Key Vault Integration](docs/vault.md) (Pro)
- [Pro Features](docs/pro-features.md)
- [SSO & RBAC](docs/sso-rbac.md) (Pro)
- [Troubleshooting](docs/troubleshooting.md)

## Architecture

```
┌─────────────────────────────────────────────┐
│         wshm-core (this repo, OSS)          │
│                                              │
│  CLI ── Daemon ── Web UI ── TUI              │
│    │        │        │       │              │
│    └────────┴────────┴───────┘              │
│              │                               │
│     12 OSS pipelines                         │
│     (triage, pr_analysis, merge_queue,       │
│      notify, changelog, dashboard,           │
│      revert, backup, pr_health, ...)         │
│              │                               │
│    ┌─────────┴─────────┐                     │
│    ▼                   ▼                     │
│  SQLite           PostgreSQL                 │
└─────────────────────────────────────────────┘
```

**Pro features** live in a separate crate (`wshm-pro`, not open source) and attach via runtime hooks.

## Why SSPL?

Like MongoDB and Elastic, wshm uses the [Server Side Public License v1](./LICENSE). You can use, modify, and self-host freely. The only restriction: if you offer wshm as a **hosted SaaS service** to third parties, you must open-source your entire service stack.

This protects the business model while keeping the core truly open for self-hosted use. 99% of users will never hit the SSPL restriction.

## Contributing

Issues and PRs welcome. For substantial changes, please open an issue first to discuss.

See [docs/getting-started.md](docs/getting-started.md) for the development setup.

## Disclaimer

**wshm is provided "AS IS", without warranty of any kind, express or implied**, including but not limited to the warranties of merchantability, fitness for a particular purpose, and noninfringement. In no event shall the authors or copyright holders be liable for any claim, damages, or other liability, whether in an action of contract, tort, or otherwise, arising from, out of, or in connection with wshm or the use or other dealings in wshm.

**You use wshm at your own risk.** wshm interacts with your GitHub/GitLab/Gitea/Azure DevOps repositories. It can create, modify, delete issues, pull requests, labels, and comments. **Always run in dry-run mode first** (without `--apply`) to verify the intended actions. The authors are not responsible for any data loss, incorrect labeling, unwanted PR merges, accidental comments, API rate-limit exhaustion, AI provider bills, or any other consequences of running this software.

**AI classifications and suggestions can be wrong.** wshm uses LLMs (Anthropic, OpenAI, Google, local models, etc.) for automated analysis. AI outputs are probabilistic, may contain hallucinations, and should never be considered authoritative. Always review AI-generated PRs, comments, and labels before trusting them in production workflows.

**Your data, your responsibility.** wshm is self-hosted. The authors have zero access to your data, tokens, API keys, or analysis results. Secure your `.wshm/` directory, protect your GitHub tokens, and follow standard security practices for your infrastructure.

By using wshm you acknowledge that you have read, understood, and agreed to these terms.

## License

[SSPL-1.0](./LICENSE) — Copyright © 2025-2026 wshm-dev
