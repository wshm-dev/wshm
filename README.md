# wshm

> Your repo's wish is my command.

**AI-powered repository agent** for GitHub, GitLab, Gitea, and Azure DevOps. Triage issues, analyze pull requests, manage merge queues, and automate daily repo hygiene — all from a single self-hosted binary.

[![License: SSPL](https://img.shields.io/badge/License-SSPL--1.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)

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
- **Daemon** — background polling with embedded web dashboard and TUI
- **Multi-repo** — manage multiple repos from one daemon instance
- **14 AI providers** — Anthropic, OpenAI, Google, Mistral, Groq, DeepSeek, xAI, Ollama (local), llama.cpp, and more
- **4 Git providers** — GitHub, GitLab, Gitea, Azure DevOps (self-hosted friendly)
- **SQLite + PostgreSQL** backends
- **OIDC SSO** — login via Google, GitHub, GitLab, Azure AD

### Pro ([wshm.dev/pro](https://wshm.dev/pro))

Token-heavy AI features and enterprise integrations:

- **Inline code review** — line-by-line AI review on PR diffs
- **Auto-fix** — generate PRs from issue descriptions (Claude Code / Codex / containerized)
- **AI conflict resolution** — automatic rebase with AI assistance
- **Improvement proposals** — analyze codebase, create refactor/testing/perf issues
- **HTML/PDF reports** — full repo health reports with SLA metrics
- **Cloud exports** — S3, Azure Blob, GCS, Elasticsearch, OpenSearch, MongoDB, MySQL
- **Vault integration** — HashiCorp Vault, AWS Secrets Manager, Azure Key Vault, GCP Secret Manager
- **SAML SSO** — Okta, Azure AD, custom IdP
- **RBAC + Audit log** — organizations, roles, action audit trail
- **Daemon webhook mode** — real-time GitHub webhook processing

## Quick Start

### Install

```bash
# macOS / Linux
curl -fsSL https://wshm.dev/install.sh | sh

# Cargo
cargo install wshm-core --bin wshm

# Docker
docker pull ghcr.io/wshm-dev/wshm:latest
```

### Configure

```bash
cd your-repo
wshm config init        # creates .wshm/config.toml
export GITHUB_TOKEN=ghp_xxxxx
export ANTHROPIC_API_KEY=sk-ant-xxxxx
wshm login              # interactive auth setup
```

### Run

```bash
wshm sync               # fetch issues + PRs from GitHub
wshm triage --apply     # classify and label open issues
wshm pr analyze --apply # analyze and label open PRs
wshm queue              # show ranked merge queue
wshm run --apply        # full cycle: sync → triage → analyze → queue
```

### Dashboard

```bash
wshm daemon             # starts polling + embedded web UI
# Open http://127.0.0.1:3000
```

Or interactive terminal UI:

```bash
wshm tui
```

## Documentation

Full guides in [`docs/`](./docs):

- [Getting Started](docs/getting-started.md)
- [Configuration](docs/configuration.md)
- [CLI Reference](docs/cli-reference.md)
- [Daemon & Web UI](docs/daemon.md)
- [License & Activation](docs/license.md)
- [Vault Integration](docs/vault.md) (Pro)
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
