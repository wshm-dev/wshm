# Configuration

wshm is configured via `.wshm/config.toml` in your repository root.

## Initialize

```bash
wshm config init
```

## Full Reference

```toml
[github]
# Token from env: GITHUB_TOKEN or WSHM_TOKEN (never in config)

[ai]
provider = "anthropic"            # anthropic, openai, google, mistral, groq, deepseek, xai, ollama, ...
model = "claude-sonnet-4-20250514"
# base_url = "https://custom-proxy.com/v1"  # optional: custom API endpoint

[ai.rag]                          # "Related history" in triage / PR-analysis prompts
enabled = true                    # closed issues & merged PRs found by full-text search
top_k = 5                         # items kept in the prompt
max_terms = 8                     # search terms extracted from title + body
max_chars = 3000                  # size cap of the block
# Env overrides (beat the file): WSHM_RAG_ENABLED, WSHM_RAG_TOP_K,
# WSHM_RAG_MAX_TERMS, WSHM_RAG_MAX_CHARS
# Per-repo switch: Settings → Repos → "Related history in prompts"
# (features.related_history, default true); both switches must be on.
# Logs: one INFO line per triage/analysis listing the retrieved items,
# the full block at RUST_LOG=debug.

[triage]
enabled = true
auto_fix = false                  # attempt auto-fix for simple bugs
auto_fix_confidence = 0.85        # minimum confidence for auto-fix
triage_confidence = 0.5           # minimum confidence to apply triage
labels_bug = "bug"
labels_feature = "feature"
labels_duplicate = "duplicate"
labels_wontfix = "wontfix"
labels_needs_info = "needs-info"
retriage_interval_hours = 0       # re-evaluate every N hours (0 = disabled)

[pr]
enabled = true
auto_label = true
risk_labels = true                # add risk:low / risk:medium / risk:high

# Architecture diagram in the PR analysis comment (Mermaid, inserted before
# the review checklist). One extra AI call per new diff, cached per PR.
# Every key can be overridden per repo in Settings → Diagrams (stored in
# the DB, so all pods share it): DB > this file > defaults.
[pr.review_diagrams]
enabled = true
kinds = ["architecture"]          # only "architecture" is implemented
text = "short"                    # short | normal | full
language = "en"                   # language of the diagram texts
comment = "mermaid"               # mermaid | none (link to the page only)
diff_viewer = true                # embed the diff in the interactive page
active_days = 14                  # skip PRs idle for longer (0 = no limit); a push re-enables
max_mermaid_chars = 5000          # Mermaid size cap (Forgejo/Gitea default limit)
timeout_secs = 120                # budget of the diagram step (runs after the comment is posted)
max_summary_calls = 3             # per-file summary calls for diffs > 40 KB

# Interactive page (React Flow viewer), deleted when the PR is closed.
# Destinations (dir, pages_repo, bucket, region, endpoint, account,
# container, public_url) and allow_private are TOML-only: the per-repo
# override from the dashboard can only choose `target` and the caps.
[pr.review_diagrams.publish]
target = "off"                    # off | local | github-pages | wshm (Enterprise) | s3 | gcs | azure
# s3/gcs/azure: bucket / region / endpoint (S3, GCS) or account / container
# (Azure), objects uploaded as text/html; needs a build with the matching
# storage-s3 / storage-gcs / storage-azure feature. Credentials come from the
# provider's standard chain (env, workload identity), never from this file.
# prefix = "wshm-diagrams"
# public_url = "https://diagrams.example.com"  # link = public_url/<key>; for wshm: the daemon URL
# dir = "/var/www/diagrams"       # local (TOML only, not in the per-repo override)
# pages_repo = "owner/diagrams"   # github-pages: commits to its gh-pages branch (TOML only).
#                                 # If this repo is public, its tree and git history list
#                                 # every page: the random token in the file name does not
#                                 # prevent enumeration there.
# retention_days = 30             # delete pages idle for longer (0 = until the PR closes)
# max_pages = 200                 # per repo, least recently updated pages evicted first
# max_diff_kb = 1024              # bigger diffs: the viewer links to "Files changed"
# allow_private = false           # public targets are refused for private repos (TOML only)

[queue]
enabled = true
merge_threshold = 15              # minimum score to auto-merge
strategy = "rebase"               # merge, rebase, squash

[conflicts]
enabled = true                    # conflict detection (read-only)
# auto_resolve / auto_resolve_confidence: not implemented, ignored

[fix]
# tool = "claude-code"            # or "codex"
# model = "..."                   # override the agent model
# docker_image = "wshm-sandbox:latest"  # used with `wshm fix --docker`
# container_network = "none"     # network for `--docker` (default none: no egress, so the model API is unreachable;
#                                 # set a network with a controlled egress). WSHM_FIX_CONTAINER_NETWORK is a fallback when empty.
# trusted_authors_only = true     # default; WARNING: false lets any issue author steer the agent
# test_command = "cargo test"
# test_retries = 2

[sync]
interval_minutes = 5              # minimum time between auto-syncs
full_sync_interval_hours = 24

[license]
# key = "vault(secret/wshm/license-key)"  # vault placeholder (recommended)
# path = "~/.wshm/license.jwt"            # or explicit JWT path
# Or use env: WSHM_LICENSE_KEY

[vault]
# provider = "hashicorp"          # hashicorp, aws, azure, gcp
# address = "https://vault.company.com"
# mount = "secret"

[web]
# enabled = true
# username = "admin"
# password = "changeme"

[database]
# provider = "sqlite"             # sqlite (default) or postgresql
# uri = "postgres://user:pass@localhost/wshm"

[notify]
# on_run = true                  # `wshm run` sends the summary (the daemon does not)
#
# [[notify.discord]]
# url = "https://discord.com/api/webhooks/ID/TOKEN"
#
# [[notify.slack]]
# url = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"
#
# [[notify.teams]]
# url = "https://outlook.office.com/webhook/YOUR/URL"

[export]
# [export.storage]                # one JSON object per event: <prefix>/YYYY/MM/DD/<Kind>-<ms>.json
# provider = "s3"                 # s3, gcs, azure, local (Enterprise)
# bucket = "wshm-events"          # s3 / gcs
# prefix = "wshm/events"
# region = "eu-west-1"             # s3
# endpoint = "https://s3.gra.io.cloud.ovh.net"  # S3-compatible (MinIO, OVH, Scaleway...)
# account = "mystorageacct"        # azure (or AZURE_STORAGE_ACCOUNT_NAME)
# container = "wshm-events"        # azure
# dir = "/var/lib/wshm/events"     # local
# Credentials are never set here: AWS_* / instance role, GOOGLE_APPLICATION_CREDENTIALS /
# workload identity, AZURE_STORAGE_* / managed identity.
# public_url = "https://cdn.example.com"  # required for links with a custom endpoint
#   (config or env AWS_ENDPOINT*, GOOGLE_BASE_URL, AZURE_STORAGE_ENDPOINT / _USE_EMULATOR)
# S3 credentials: env, web identity, ECS or instance role only (no AWS_PROFILE / ~/.aws/config / SSO).
# A misconfigured sink (missing bucket, invalid prefix...) is logged as an error and skipped;
# wshm still starts.
#
# [export.database]
# provider = "elastic"            # elastic, opensearch, mongodb, mysql (Enterprise)
# uri = "http://localhost:9200"
#
# [[export.webhooks]]
# url = "https://your-server.com/wshm"
# events = ["*"]
# secret = "hmac-secret"

[branding]
# name = "wshm"
# command_prefix = "/wshm"

[labels_blacklist]
# Labels wshm must never apply

[issues_blacklist]
# Issue numbers to never touch

[prs_blacklist]
# PR numbers to never touch
```

## Auto-fix and code execution

The claude agent used by auto-fix may only read and edit files, plus run Bash for the configured `[fix] test_command`. That command still executes repository code (build scripts, test code, package hooks), and the agent can edit that code before running it. This is why `trusted_authors_only = true` (the default) and `wshm fix --docker` (with `container_network = "none"` by default) matter: keep both unless every issue author is trusted.

## Pipelines

Each issue or PR event (webhook or poller) is processed by a **pipeline**: an
ordered list of steps. Without any pipeline config, the built-in `standard`
pipeline runs, and it does exactly what the repo toggles say:

| Item  | Steps of `standard`                                                    |
|-------|------------------------------------------------------------------------|
| issue | `sync` (collect_issues) → `triage` (triage_issues + filters) → `memory` |
| PR    | `sync` (collect_prs) → `review` (review_prs, a failure does not stop the run) → `analyze` (analyze_prs + filters) → `memory` |

So the existing toggles keep working unchanged: `standard` reads
`collect_issues`, `collect_prs`, `triage_issues`, `review_prs`, `analyze_prs`
and the repo filters as step conditions; `review_post_comments` and the apply
mode still decide what the steps post. `auto_pr` and `auto_merge` are not part
of the event pipeline (they were not before either).

**A custom template does not apply the repo toggles by itself.** A step runs
when its own `when` holds: to keep honouring a toggle, list it, e.g.
`when = { features = ["triage_issues"], repo_filters = true }`. Without it, the
step runs even when the toggle is off in Settings → Repos.

The same template drives the other entry points, as a partial run (only the
steps of one kind, recorded with their own trigger):

- the scheduler's periodic triage and retriage run the `triage` steps of each
  pending issue's template (trigger `scheduler`), still behind `[triage]
  enabled`, `triage_issues` and apply mode as before. Such a run is recorded
  only when a step actually runs. `needs` on steps outside the partial run
  (e.g. `sync`) count as met: the scheduler synced already;
- the slash commands `/wshm triage`, `analyze` and `review` run the steps of
  that kind (trigger `command`). Their conditions apply, except the toggles
  (`features`, `repo_filters`), which govern automatic processing and never
  blocked an explicit command. A command on an item whose template has no such
  step, or skips it, answers with the reason. The other commands (`label`,
  `fix`, `queue`, ...) are not pipeline steps.

The pipeline config is only read from `.wshm/config.toml` (default branch
checkout), the global config, or the DB override. Never from the content of an
issue or PR. It holds no secret: steps read credentials from the secret store.

```toml
[[pipelines]]
name = "light"                  # a-z 0-9 - _ (1-64 chars); "standard" is reserved
description = "Docs and small PRs"
max_ai_calls = 2                # AI call budget of a run; later steps are skipped

[[pipelines.steps]]
id = "sync"                     # unique in the pipeline
uses = "sync"                   # step kind

[[pipelines.steps]]
id = "analyze"
uses = "analyze"
needs = ["sync"]                # earlier steps that must have finished "done"
on_failure = "continue"         # "abort" (default): stop the run and fail the event
timeout_secs = 300              # the step fails after this long
when = { item = "pr", features = ["analyze_prs"], labels_none = ["skip-ai"] }

[[pipeline_routes]]             # picks the pipeline of a run
template = "light"
priority = 10                   # highest first, then declaration order
when = { item = "pr", paths = ["docs/**", "**/*.md"] }

[pipeline_runs]                 # run history retention
retention_days = 30             # 0 = no age limit
max_per_item = 20               # runs kept per issue/PR, newest first (0 = no limit)
```

No route matches: `standard` runs.

Step kinds: `sync` (refresh the issue/PR from the forge), `triage` (issues),
`review` (PRs, inline review), `analyze` (PRs, analysis and diagrams),
`memory` (store the result in ICM when `[daemon] icm_enabled`). A step of the
other item kind is left out of the run.

Conditions (`when`, on steps and routes; all listed must hold):

| Key | Meaning |
|-----|---------|
| `item` | `"issue"` or `"pr"` |
| `features` | repo toggles that must all be on, e.g. `["triage_issues"]` |
| `repo_filters` | apply the repo filters (skip_authors, triage labels/age, target_branches) |
| `labels_any` / `labels_none` | labels on the item (case-insensitive) |
| `authors` / `skip_authors` | item author logins |
| `forges` | `github`, `gitlab`, `gitea`, `forgejo`, `azure-devops` |
| `paths` | PR only: a changed path matches a glob (`*`, `**`, `?`) |
| `risk` | PR only: risk level of the analysis (`low`, `medium`, `high`) |
| `repos` | repo slug globs, e.g. `["my-org/*"]` |

A step that does not run is recorded as skipped with the reason
(`review_prs disabled`, `needs 'sync-pr', which was skipped`, ...).

Routes are evaluated before any step, so on a webhook they read the labels and
author from the event payload (a label just added by `pull_request.labeled`
matches); step conditions read the cached item, refreshed by the `sync` step.

Errors never stop the daemon from booting. An invalid TOML pipeline config is
logged and the repo runs `standard` (a valid `[pipeline_runs]` is kept). A
pipeline using an unknown step kind falls back to `standard` (logged). An
unknown name in `when.features` (for instance a toggle renamed by an upgrade) is
a warning and evaluates to false. `GET /api/v1/repos/{slug}/pipelines` shows
them all: `toml_error`, `override_error`, `template_errors`, `warnings`.

Limits:

- `timeout_secs` stops the step where it is: what it already did stays (a label
  set, a comment posted), so partial effects are possible. It is safest on
  `sync` and `memory`; on `triage`, `review` and `analyze` the config gets a
  warning.
- `max_ai_calls` is checked before each step, so the step that crosses the
  budget finishes. The `analyze` step's diagram runs in the background after the
  step: it is not started once the budget is used up, but a started diagram
  (up to `[pr.review_diagrams] max_summary_calls` + 1 calls) is not cut short.
  Its calls are added to the run when it finishes.
- A failure that the wrapped pipeline already handled by logging and going on
  (a triage or review AI error) is recorded as `failed` but does not abort the
  run, as before.

DB override (multi-pod, shared by every pod): `PUT /api/v1/repos/{slug}/pipelines`
(admin) with a JSON object holding any of `pipelines`, `pipeline_routes`,
`pipeline_runs`; each replaces the TOML value (`pipeline_runs` is merged key by
key). `null` drops the override. The PUT rejects an invalid override; one
that became invalid later (for instance after an upgrade) only loses its
invalid sections at run time (`override_error`). `GET` on the same path returns
the effective, TOML and override values, the errors and warnings, and the
built-in templates.

Runs: `GET /api/v1/repos/{slug}/runs?item=N` (optionally `&kind=issue|pr`,
`&limit=50`) returns the runs, newest first: template, trigger, PR head sha,
status, and per step the status (done / skipped / failed), reason, timings,
AI calls, summary and findings count.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` | GitHub personal access token |
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key (if using OpenAI) |
| `WSHM_LICENSE_KEY` | License key (alternative to vault/config) |
| `WSHM_TOKEN` | Alternative to GITHUB_TOKEN |
| `WSHM_RAG_ENABLED` | Override `[ai.rag] enabled` (true/false) |
| `WSHM_RAG_TOP_K` | Override `[ai.rag] top_k` |
| `WSHM_RAG_MAX_TERMS` | Override `[ai.rag] max_terms` |
| `WSHM_RAG_MAX_CHARS` | Override `[ai.rag] max_chars` |
| `VAULT_TOKEN` | HashiCorp Vault token |
| `VAULT_ROLE_ID` | HashiCorp AppRole role ID |
| `VAULT_SECRET_ID` | HashiCorp AppRole secret ID |
| `AWS_ACCESS_KEY_ID` | AWS credentials |
| `AWS_SECRET_ACCESS_KEY` | AWS credentials |
| `AZURE_CLIENT_ID` | Azure service principal |
| `AZURE_CLIENT_SECRET` | Azure service principal |
| `AZURE_TENANT_ID` | Azure tenant |
| `GOOGLE_APPLICATION_CREDENTIALS` | GCP service account JSON path |

## Global Config (Multi-Repo)

For managing multiple repos with the daemon, use `~/.wshm/global.toml`:

```toml
[daemon]
bind = "127.0.0.1:3000"
webhook_secret = "your-secret"
apply = false
poll = true
poll_interval = 30

[[repos]]
slug = "owner/repo-1"
path = "/home/user/repos/repo-1"
enabled = true

[[repos]]
slug = "owner/repo-2"
path = "/home/user/repos/repo-2"
enabled = true
apply = true    # override per-repo
```

## Config Precedence

1. CLI flags (`--apply`, `--offline`, `--repo`)
2. Environment variables
3. `.wshm/config.toml` (per-repo)
4. `~/.wshm/global.toml` (global defaults)
5. Built-in defaults
