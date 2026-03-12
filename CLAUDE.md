# wshm — CLAUDE.md

> AI-powered GitHub agent for OSS maintainers.
> Triage issues, auto-fix simple ones, analyze PRs, resolve conflicts.
> Built in Rust. Zero infra. One binary.

---

## Project Overview

**wshm** (wishmaster) is a CLI tool + GitHub Action that acts as an autonomous repo maintenance agent.
It runs on every new issue or PR event, and on a schedule for conflict detection.

All state is persisted in `.wshm/state.db` (SQLite), committed automatically to the repo.
No external services. No accounts. No infrastructure.

### Tagline
"Your repo's wish is my command."

---

## .wshm/ Directory

```
.wshm/
├── config.toml      ← user config (committed by user, never overwritten by wshm)
├── state.db         ← SQLite state (committed automatically by wshm after each run)
└── logs/            ← run logs (not committed, in .gitignore)
    └── 2026-03-11.log
```

**In the target repo's .gitignore:**
```gitignore
.wshm/logs/
```

`state.db` MUST be committed — it is the persistence layer between runs.

---

## Local SQLite Cache (Critical Architecture Decision)

wshm maintains a local SQLite database (`.wshm/state.db`) that mirrors GitHub state.
This avoids hammering the GitHub API (rate limit: 5000 req/h) and makes everything instant locally.

### Cache Strategy

```
GitHub API ──sync──▶ SQLite (.wshm/state.db) ──read──▶ wshm pipelines
                          │
                          ├── issues (number, title, body, labels, state, created_at, updated_at)
                          ├── pull_requests (number, title, body, labels, state, head_sha, base_sha, mergeable, ci_status)
                          ├── comments (id, issue_number, body, author, created_at)
                          ├── labels (name, color, description)
                          ├── triage_results (issue_number, classification, confidence, acted_at)
                          ├── pr_analyses (pr_number, summary, risk_level, type, analyzed_at)
                          └── sync_log (table_name, last_synced_at, etag)
```

### Sync Rules

1. **Incremental sync** — use `If-None-Match` (ETag) and `since` parameter to fetch only changes
2. **Sync before action** — every `wshm` command starts with a lightweight sync (< 2 API calls if nothing changed)
3. **Full sync** — `wshm sync` forces a full refresh of all tables
4. **Offline mode** — `wshm --offline` skips sync entirely, works from cache only
5. **Write-through** — when wshm applies an action (label, comment, close), it updates both GitHub AND the local cache atomically
6. **ETag tracking** — store GitHub ETags per endpoint in `sync_log` table to minimize API calls
7. **Conditional requests** — GitHub returns 304 Not Modified when nothing changed = 0 rate limit cost

### Example Sync Flow

```
wshm triage --apply
    │
    ▼
[1] Check sync_log for issues table
    ├── last_synced < 5 min ago? → skip sync, use cache
    └── else → GET /repos/{owner}/{repo}/issues?since={last_synced}&state=open
               with If-None-Match: {stored_etag}
               ├── 304 Not Modified → update sync timestamp, use cache
               └── 200 OK → upsert changed issues into SQLite, store new ETag
    │
    ▼
[2] Read issues from SQLite (instant, no network)
    │
    ▼
[3] Run AI classification on untriaged issues
    │
    ▼
[4] Apply labels + comment on GitHub
    │
    ▼
[5] Update triage_results in SQLite + update issue labels in cache
```

---

## Core Workflow (The 4 Pipelines)

### Pipeline 1 — Issue Triage + Auto-fix
Triggered: `issues.opened` event or `wshm triage`

```
New Issue
    │
    ▼
[1] Classify (from SQLite cache)
    ├── duplicate?  → find original, close with link
    ├── wontfix?    → explain politely, close
    ├── needs-info? → ask for missing info, label
    ├── bug         → label, prioritize
    └── feature     → label, add to backlog
    │
    ▼
[2] Simple fix attempt (bugs only, confidence > 0.85)
    ├── Search codebase for relevant files
    ├── Generate fix with Claude
    ├── Open a PR with fix + "fixes #<issue_number>"
    └── Comment on issue: "I've opened PR #X with a potential fix"
    │
    ▼
[3] Post triage comment on issue
    └── Summary: category, priority, next steps
```

**Simple = fixable in 1-3 files, no architecture change, clear repro**

---

### Pipeline 2 — PR Analysis + Labeling
Triggered: `pull_request.opened` + `pull_request.synchronize` events or `wshm pr analyze`

```
New/Updated PR
    │
    ▼
[1] Fetch diff + linked issues + CI status (from cache + targeted API call for diff)
    │
    ▼
[2] AI Analysis
    ├── Summary: what does this PR do? (2-3 sentences)
    ├── Risk level: low / medium / high
    ├── Type: bug-fix / feature / refactor / docs / chore
    ├── Linked issues: auto-detect "fixes #X" patterns
    └── Review checklist: tests present? breaking change? docs updated?
    │
    ▼
[3] Apply labels + post analysis comment
    │
    ▼
[4] Store analysis in pr_analyses table
```

---

### Pipeline 3 — Merge Queue
Triggered: `wshm queue`

```
Open PRs (from SQLite cache)
    │
    ▼
[1] Score each PR
    ├── CI passing?        (+10)
    ├── Approved reviews?  (+5 per approval)
    ├── Conflicts?         (-10)
    ├── Age (staleness)    (+1 per day, max +10)
    ├── Risk level         (low: +5, medium: 0, high: -5)
    └── Linked to issue?   (+3)
    │
    ▼
[2] Rank by score
    │
    ▼
[3] Output ranked list (dry-run)
    └── With --apply: merge top PR if score > threshold
```

---

### Pipeline 4 — Conflict Resolution
Triggered: schedule or `wshm conflicts scan`

```
Open PRs (from SQLite cache)
    │
    ▼
[1] For each PR: check mergeable status (cached, refreshed on sync)
    │
    ▼
[2] Conflicting PRs
    ├── Attempt rebase from main
    ├── If conflict: AI resolution (confidence > 0.85)
    ├── Push new commit (NEVER force-push)
    └── Comment on PR: "Resolved conflicts with main in commit abc123"
    │
    ▼
[3] Report: list of PRs with conflict status
```

---

## CLI Design

```
wshm                           # show status (from cache, instant)
wshm sync                      # force full sync from GitHub
wshm triage [--issue <N>]      # classify issues [or single issue]
wshm triage --apply            # classify + label + comment
wshm pr analyze [--pr <N>]     # analyze PRs [or single PR]
wshm pr analyze --apply        # analyze + label + comment
wshm queue                     # show ranked merge queue
wshm queue --apply             # merge top PR if above threshold
wshm conflicts scan            # detect conflicting PRs
wshm conflicts scan --apply    # attempt resolution
wshm run                       # full cycle: sync + triage + analyze + queue + conflicts
wshm run --apply               # full cycle with actions
wshm config init               # create .wshm/config.toml template
```

**Global flags:**
- `--apply` — actually perform actions (dry-run by default)
- `--offline` — skip GitHub sync, use cached data only
- `--verbose` / `-v` — detailed output
- `--json` — JSON output for scripting
- `--repo <owner/repo>` — override detected repo

---

## Config: .wshm/config.toml

```toml
[github]
# Token from env var GITHUB_TOKEN or WSHM_TOKEN (never stored in config)

[ai]
provider = "anthropic"           # "anthropic" | "openai"
model = "claude-sonnet-4-20250514"
# API key from env var ANTHROPIC_API_KEY (never stored in config)

[triage]
enabled = true
auto_fix = false                 # attempt auto-fix for simple bugs
auto_fix_confidence = 0.85       # minimum confidence for auto-fix
labels_bug = "bug"
labels_feature = "feature"
labels_duplicate = "duplicate"
labels_wontfix = "wontfix"
labels_needs_info = "needs-info"

[pr]
enabled = true
auto_label = true
risk_labels = true               # add risk:low / risk:medium / risk:high

[queue]
enabled = true
merge_threshold = 15             # minimum score to auto-merge
strategy = "rebase"              # "merge" | "rebase" | "squash"

[conflicts]
enabled = true
auto_resolve = false             # attempt AI conflict resolution
auto_resolve_confidence = 0.85

[sync]
interval_minutes = 5             # minimum time between auto-syncs
full_sync_interval_hours = 24    # force full sync every N hours
```

---

## Project Structure

```
wshm/
├── Cargo.toml
├── CLAUDE.md                    ← this file
├── README.md
├── LICENSE                      ← MIT
├── action.yml                   ← GitHub Action definition
├── src/
│   ├── main.rs                  ← CLI entry point (clap)
│   ├── lib.rs                   ← public API
│   ├── config.rs                ← TOML config parsing
│   ├── db/
│   │   ├── mod.rs               ← SQLite connection + migrations
│   │   ├── schema.rs            ← table definitions + migrations
│   │   ├── issues.rs            ← issue CRUD operations
│   │   ├── pulls.rs             ← PR CRUD operations
│   │   ├── sync.rs              ← sync log + ETag management
│   │   └── triage.rs            ← triage results storage
│   ├── github/
│   │   ├── mod.rs
│   │   ├── client.rs            ← octocrab wrapper with retry + rate limit
│   │   ├── sync.rs              ← incremental sync logic (ETag, since)
│   │   ├── issues.rs            ← fetch/label/comment/close issues
│   │   ├── pulls.rs             ← fetch PRs, diff, CI status
│   │   └── git.rs               ← clone, rebase, push (via git2)
│   ├── ai/
│   │   ├── mod.rs
│   │   ├── client.rs            ← Claude/OpenAI API client
│   │   ├── prompts/
│   │   │   ├── issue_classify.rs
│   │   │   ├── pr_analyze.rs
│   │   │   ├── conflict_resolve.rs
│   │   │   └── issue_fix.rs
│   │   └── schemas.rs           ← structured output types
│   └── pipelines/
│       ├── mod.rs
│       ├── triage.rs            ← Pipeline 1
│       ├── pr_analysis.rs       ← Pipeline 2
│       ├── merge_queue.rs       ← Pipeline 3
│       └── conflict_resolution.rs  ← Pipeline 4
└── tests/
    ├── fixtures/                ← sample GitHub API responses
    └── integration/
```

---

## Tech Stack

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }       # CLI parsing
octocrab = "0.44"                                      # GitHub API
rusqlite = { version = "0.32", features = ["bundled"] } # SQLite (bundled = no system dep)
reqwest = { version = "0.12", features = ["json"] }    # HTTP for AI APIs
serde = { version = "1", features = ["derive"] }       # Serialization
serde_json = "1"
toml = "0.8"                                           # Config parsing
tokio = { version = "1", features = ["full"] }         # Async runtime
tracing = "0.1"                                        # Logging
tracing-subscriber = "0.3"
git2 = "0.19"                                          # Git operations
chrono = { version = "0.4", features = ["serde"] }     # Timestamps
anyhow = "1"                                           # Error handling
```

---

## AI Integration Pattern

All AI calls follow the same pattern:

```rust
// 1. Build prompt with context from SQLite cache
let prompt = prompts::issue_classify::build(&issue, &similar_issues);

// 2. Call AI API
let response = ai_client.complete(&prompt).await?;

// 3. Parse structured response (JSON)
let classification: IssueClassification = serde_json::from_str(&response)?;

// 4. Validate confidence threshold
if classification.confidence < config.triage.auto_fix_confidence {
    tracing::info!("Low confidence ({:.2}), skipping auto-action", classification.confidence);
    return Ok(Action::ReportOnly(classification));
}

// 5. Execute action (if --apply)
if args.apply {
    github.label_issue(issue.number, &classification.label).await?;
    github.comment_issue(issue.number, &classification.summary).await?;
    // Update local cache
    db.upsert_triage_result(&classification)?;
}
```

**AI response format (enforced via system prompt):**
```json
{
  "category": "bug",
  "confidence": 0.92,
  "priority": "high",
  "summary": "Memory leak in connection pool when...",
  "suggested_labels": ["bug", "priority:high", "area:networking"],
  "is_duplicate_of": null,
  "is_simple_fix": true,
  "relevant_files": ["src/pool.rs", "src/connection.rs"]
}
```

---

## Safety Principles

1. **Dry-run by default** — no `--apply` = only prints what would happen
2. **Confidence gates** — never act autonomously below threshold (default 0.85)
3. **Never force-push** — conflict resolution uses new commits, not force-push
4. **Idempotent** — re-running same command = same result, no duplicate comments
5. **Token is sacred** — always from env var, never logged, never in config files
6. **Transparent** — every autonomous action posts a comment explaining what and why
7. **Cache-first** — minimize API calls, maximize local speed

---

## Build Milestones

### M1 — SQLite Cache + Issue Triage (START HERE)
- [x] `db::schema` — SQLite migrations (issues, labels, sync_log, triage_results)
- [x] `db::issues` — CRUD for issues table
- [x] `db::sync` — sync log + ETag storage
- [x] `github::client` — octocrab wrapper with retry + rate limit handling
- [x] `github::sync` — incremental sync (ETag + since parameter)
- [x] `github::issues` — fetch open issues, post comment, apply label
- [x] `ai::client` — Claude API call, parse JSON response
- [x] `ai::prompts::issue_classify` — classification prompt
- [x] `ai::schemas` — IssueClassification struct
- [x] `pipelines::triage` — full Pipeline 1 step 1
- [x] `wshm sync` — sync command
- [x] `wshm triage --issue <N> [--apply]` — triage single issue
- [x] `wshm triage [--apply]` — triage all open issues
- [x] Tests with fixtures (mock GitHub API responses in SQLite)
- [ ] README with install + usage

### M2 — PR Analysis
- [x] `db::pulls` — PR CRUD + pr_analyses table
- [x] `github::pulls` — fetch PR, diff, CI status
- [x] `github::sync` — extend sync to PRs
- [x] `ai::prompts::pr_analyze` — analysis prompt
- [x] `pipelines::pr_analysis` — full Pipeline 2
- [x] `wshm pr analyze --pr <N> [--apply]`

### M3 — Conflict Resolution
- [x] `github::git` — clone repo, detect conflicts, rebase via git2
- [x] `ai::prompts::conflict_resolve` — resolution prompt
- [ ] `pipelines::conflict_resolution` — full Pipeline 4 (stub only)
- [ ] `wshm conflicts scan [--apply]` (stub only)

### M4 — Merge Queue + Auto-fix + Full Cycle
- [x] `pipelines::merge_queue` — scoring + ranking
- [x] `ai::prompts::issue_fix` — fix generation prompt
- [x] `wshm queue [--apply]` (scoring done, merge not yet)
- [x] `wshm run [--apply]` — full cycle
- [x] GitHub Action (`action.yml`)

**Start with M1. Ship each milestone before moving to next.**

---

## Definition of Done (per milestone)

- [x] All commands work in dry-run mode
- [x] All commands work with --apply
- [x] All commands work with --offline (from cache)
- [ ] All commands produce --json output
- [x] SQLite cache is updated on every action
- [x] Tests pass with fixture data (no real API calls in tests)
- [x] No panics — all errors handled with anyhow
- [x] `cargo clippy` clean (dead code warnings only for future milestones)
- [x] `cargo fmt` applied
