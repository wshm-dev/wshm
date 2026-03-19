<p align="center">
  <h1 align="center">🧞 wshm</h1>
  <p align="center"><strong>Your repo's wish is my command.</strong></p>
  <p align="center">AI-powered GitHub agent for repository maintenance.<br>Triage issues, review PRs, auto-fix bugs, manage merge queues — all from a single binary.</p>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#how-it-works">How it works</a> •
  <a href="#pipelines">Pipelines</a> •
  <a href="#configuration">Configuration</a> •
  <a href="#early-access">Early Access</a>
</p>

<p align="center">
  <a href="#-english">English</a> •
  <a href="#-français">Français</a> •
  <a href="#-español">Español</a> •
  <a href="#-deutsch">Deutsch</a> •
  <a href="#-日本語">日本語</a> •
  <a href="#-中文">中文</a> •
  <a href="#-한국어">한국어</a> •
  <a href="#-português">Português</a>
</p>

---

## Features

- **🔍 Issue Triage** — Automatically classify, label, and prioritize new issues using AI
- **📊 PR Analysis** — Summarize PRs, assess risk, generate review checklists
- **🔧 Auto-Fix** — Generate and open draft PRs for simple bugs (confidence-gated)
- **📋 Merge Queue** — Score and rank PRs by readiness, auto-merge when above threshold
- **🔀 Conflict Resolution** — Detect and auto-resolve merge conflicts (never force-pushes)
- **📝 Inline Review** — AI-powered line-by-line code review comments
- **👥 Auto-Assign** — Weighted random assignment of maintainers to issues and PRs
- **🏷️ Labels Blacklist** — Prevent specific labels from ever being applied
- **🔄 Periodic Retriage** — Re-evaluate stale triage results on a schedule
- **📈 Dashboard & Reports** — HTML dashboards and markdown/PDF reports
- **🎨 Fully Customizable** — Templates for every comment, branding, and behavior

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
Open PRs → Score (CI ✓, reviews, age, risk, conflicts) → Ranked list
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

| Provider | Env Variable | Local |
|----------|-------------|-------|
| Anthropic | `ANTHROPIC_API_KEY` | — |
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
| Ollama | — | ✅ |

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
> 📧 **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)**
>
> We're looking for early adopters to shape the product. Open-source maintainers and small teams welcome.

---

## 🇫🇷 Français

### wshm — Agent IA pour la maintenance de repos GitHub

wshm est un outil CLI + GitHub Action qui agit comme un agent autonome de maintenance de repos.

**Fonctionnalités :**
- 🔍 **Triage automatique** des issues (classification, labelling, priorité)
- 📊 **Analyse de PRs** (résumé, risque, checklist de review)
- 🔧 **Auto-fix** pour les bugs simples (PR draft avec seuil de confiance)
- 📋 **Merge queue** avec scoring et auto-merge
- 🔀 **Résolution de conflits** par IA (jamais de force-push)
- 👥 **Auto-assignation** pondérée des maintainers

Un seul binaire. Vos clés API. Vos données. Zéro infrastructure.

📧 **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)** — Accès anticipé disponible.

---

## 🇪🇸 Español

### wshm — Agente IA para el mantenimiento de repos GitHub

wshm es una herramienta CLI + GitHub Action que actúa como agente autónomo de mantenimiento.

**Características:**
- 🔍 Triaje automático de issues
- 📊 Análisis de PRs con evaluación de riesgo
- 🔧 Auto-fix para bugs simples
- 📋 Cola de merge con scoring
- 🔀 Resolución de conflictos por IA

Un solo binario. Tus claves API. Tus datos.

📧 **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)** — Acceso anticipado disponible.

---

## 🇩🇪 Deutsch

### wshm — KI-Agent für GitHub-Repository-Wartung

wshm ist ein CLI-Tool + GitHub Action, das als autonomer Agent für die Repository-Wartung fungiert.

**Funktionen:**
- 🔍 Automatische Issue-Triage
- 📊 PR-Analyse mit Risikobewertung
- 🔧 Auto-Fix für einfache Bugs
- 📋 Merge-Queue mit Scoring
- 🔀 KI-gestützte Konfliktlösung

Eine einzige Binary. Ihre API-Schlüssel. Ihre Daten.

📧 **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)** — Early Access verfügbar.

---

## 🇯🇵 日本語

### wshm — GitHubリポジトリ管理のためのAIエージェント

wshmは、リポジトリの自律的なメンテナンスエージェントとして機能するCLIツール + GitHub Actionです。

**機能：**
- 🔍 Issueの自動トリアージ（分類、ラベリング、優先度設定）
- 📊 PRの分析（要約、リスク評価、レビューチェックリスト）
- 🔧 シンプルなバグの自動修正
- 📋 スコアリング付きマージキュー
- 🔀 AIによるコンフリクト解決

単一バイナリ。あなたのAPIキー。あなたのデータ。

📧 **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)** — アーリーアクセス受付中

---

## 🇨🇳 中文

### wshm — GitHub仓库维护AI代理

wshm是一个CLI工具 + GitHub Action，作为自主仓库维护代理运行。

**功能：**
- 🔍 Issue自动分类（分类、标签、优先级）
- 📊 PR分析（摘要、风险评估、审查清单）
- 🔧 简单Bug自动修复
- 📋 带评分的合并队列
- 🔀 AI冲突解决

单一二进制文件。您的API密钥。您的数据。零基础设施。

📧 **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)** — 早期访问开放中

---

## 🇰🇷 한국어

### wshm — GitHub 저장소 관리를 위한 AI 에이전트

wshm은 자율적인 저장소 유지보수 에이전트로 작동하는 CLI 도구 + GitHub Action입니다.

**기능：**
- 🔍 이슈 자동 분류 (분류, 라벨링, 우선순위)
- 📊 PR 분석 (요약, 위험 평가, 리뷰 체크리스트)
- 🔧 간단한 버그 자동 수정
- 📋 스코어링 기반 머지 큐
- 🔀 AI 충돌 해결

단일 바이너리. 당신의 API 키. 당신의 데이터.

📧 **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)** — 얼리 액세스 가능

---

## 🇧🇷 Português

### wshm — Agente IA para manutenção de repos GitHub

wshm é uma ferramenta CLI + GitHub Action que atua como agente autônomo de manutenção de repositórios.

**Funcionalidades:**
- 🔍 Triagem automática de issues
- 📊 Análise de PRs com avaliação de risco
- 🔧 Auto-fix para bugs simples
- 📋 Fila de merge com scoring
- 🔀 Resolução de conflitos por IA

Um único binário. Suas chaves API. Seus dados.

📧 **[contact@rtk-ai.app](mailto:contact@rtk-ai.app)** — Acesso antecipado disponível.

---

<p align="center">
  <sub>Built with Rust. Zero infra. One binary.</sub><br>
  <sub>© 2026 <a href="https://rtk-ai.app">rtk-ai</a></sub>
</p>
