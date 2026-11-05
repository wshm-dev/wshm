# Open source vs Enterprise

wshm follows an open-core model:

- **Open source** (`wshm`, Apache-2.0): every feature on this page except the Enterprise section. No license key needed.
- **Enterprise (self-hosted)** (`wshm-pro`): everything in open source, plus the user management page and PostgreSQL user store, PostgreSQL multi-pod, vault placeholders and PostgreSQL secret store, exports, hosted diagram pages and support. Contact us at [contact@wshm.dev](mailto:contact@wshm.dev).

The commands below use the `wshm` binary name. The Enterprise binary is `wshm-pro` and accepts the same commands (the Docker image installs it as `wshm`).

## Upgrading from an open-source release before the open-core split

Some settings had no effect in the open-source `wshm` binary and now run. Check them before upgrading:

- `review_prs = true` (repo features): inline review runs on PR events. With the daemon in apply mode and `review_post_comments = true` (the default), review comments are posted on the PR.
- `[triage] auto_fix = true`: `wshm triage --apply` (and the daemon in apply mode) now starts auto-fix and opens draft PRs.
- `[notify] on_run = true`: `wshm run` now sends the notification summary.
- `wshm run` now also runs conflict detection (read-only).

`wshm-pro` installs are not affected: these settings already ran there.

# Open-source features

## Action Plan

AI-ranked to-do list of open issues across all monitored repos, ordered by triage priority.

```bash
# Show action plan (uses triage results from last sync)
wshm plan

# JSON output
wshm plan --json

# CSV for spreadsheets
wshm plan --csv

# Limit items per repo
wshm plan --limit 20
```

**What it does:**
- Queries all open issues with their triage results
- Ranks by priority: critical → high → medium → low
- Shows: priority, repo, issue #, category, age, simple-fix indicator, title + AI summary
- ⚡ marks issues eligible for `wshm fix` auto-fix

**Web UI:** `/improve` tab — same data in card format.

**TUI:** `Action` tab — same data in interactive table.

---


## Code Review

AI-powered inline code review on PR diffs.

```bash
# Review a specific PR
wshm review --pr 42

# Review and post comments to GitHub
wshm review --pr 42 --apply
```

**What it does:**
- Analyzes each file in the diff
- Generates inline comments with severity (error/warning/info)
- Categories: security, bug, performance, race-condition, resource-leak, error-handling, logic
- Posts review comments directly on the PR
- Warns on large PRs (>500 lines) and huge PRs (>1500 lines)

**Web UI:** `/review` tab — shows all reviewed PRs with risk level, type, and summary.

**TUI:** `Review` tab (press Tab to navigate).

---

## Auto-Fix

Generate a draft fix PR from an issue description.

```bash
# Dry run: show the prompt and the tool that would run
wshm fix --issue 15

# Generate the fix and open a draft PR
wshm fix --issue 15 --apply

# Run the agent in a rootless Podman container
wshm fix --issue 15 --docker --apply
```

**What it does:**
1. Reads the issue from the cache and checks out a clean `wshm/fix-<N>` branch from `[fix] base_branch`, in the repository root.
2. Runs the coding agent (`[fix] tool`: `claude-code` or `codex`, `[fix] model`) in the repository root:
   - claude: `--permission-mode acceptEdits`, only Read / Grep / Glob / Edit / Write tools, Bash only for `[fix] test_command`, no web tools, no MCP servers, no session persistence;
   - codex: `codex exec --sandbox workspace-write` with network access off;
   - the prompt is sent on stdin; the agent gets PATH, HOME and the model API variables only (never the forge token or `DATABASE_URL`);
   - `--docker`: `[fix] docker_image`, `--network=none`, all capabilities dropped, no-new-privileges, memory and pids limits, only the repository mounted. With `--network=none` the agent cannot reach the model API: set `WSHM_FIX_CONTAINER_NETWORK` to a network with a controlled egress, or run without `--docker`.
3. Stages every change (new files included) and rejects the fix if it touches CI or build files (`.github/`, `.gitlab-ci.yml` and other CI configs, `build.rs`, `package.json`, `Makefile`, `Dockerfile`, …).
4. Scans the staged diff for suspicious patterns (network calls, process execution, secrets) when `[fix] scan_diff = true` (default).
5. Optionally runs `[fix] test_command` with `[fix] test_retries`.
6. Commits, pushes the branch and opens a draft PR, then comments on the issue.

**Eligible issues:** bugs with `is_simple_fix = true` and confidence >= 85%.

**Web UI:** `/autofix` tab — shows eligible candidates.

**Config:**
```toml
[triage]
auto_fix = true             # triggers auto-fix from triage (with --apply)
auto_fix_confidence = 0.85

[fix]
tool = "claude-code"        # or "codex"
test_command = "cargo test"
test_retries = 2
trusted_authors_only = true # default: only fix issues opened by collaborators
```

> **Warning:** auto-fix runs an AI coding agent on text written by the issue author. Keep `trusted_authors_only = true` (the default) and draft PRs on, and review every generated PR. Turning `trusted_authors_only` off lets anyone who can open an issue steer the agent.

---

## Conflict Detection

Detect conflicting PRs.

```bash
wshm conflicts
```

**What it does:**
- Scans all open PRs for `mergeable = false`
- Groups PRs by clean / conflicting / unknown status

wshm does not rebase or resolve conflicts. `--apply` is accepted for compatibility and changes nothing. `[conflicts] auto_resolve` is not implemented and ignored.

**Web UI:** `/conflicts` tab — summary cards + conflict table.

---

## Improvement Proposals

AI-analyzed codebase improvement suggestions.

```bash
# Generate suggestions
wshm improve

# Generate and create GitHub issues
wshm improve --apply
```

**What it does:**
- Analyzes codebase structure, open issues, and recent triages
- Suggests improvements: refactor, performance, testing, security, feature, docs
- Estimates effort: trivial (<1h), small (1-4h), medium (4-16h)
- Creates GitHub issues with effort labels

**Web UI:** `/improve` tab — card list with category/effort badges.

---

## Changelog Generation

Auto-generate changelogs from merged PRs.

```bash
# Last 7 days
wshm changelog --days 7

# Markdown format
wshm changelog --days 30 --format markdown

# JSON format
wshm changelog --days 30 --format json
```

**Sections:** Features, Bug Fixes, Refactoring, Documentation, Maintenance, Other.

Categorizes by conventional commit prefix (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`) or by PR labels.

**Web UI:** `/changelog` tab — grouped sections with PR entries.

---

## Reports

Comprehensive repo health reports with metrics.

```bash
# JSON report
wshm report --format json

# Interactive HTML report with charts
wshm report --format html
```

**Includes:**
- Open issues/PRs counts with untriaged/unanalyzed
- SLA metrics (average age, items > 7d, > 30d)
- Triage category distribution
- PR risk distribution
- PR-to-issue linking
- Health analysis (duplicate, stale, zombie PRs)

**Web UI:** `/reports` tab — per-repo health dashboard with SLA cards and distribution charts.

---

## Dashboard Export

Export time-series metrics as an interactive HTML dashboard.

```bash
wshm dashboard
```

Generates `wshm-dashboard.html` with Chart.js graphs:
- Issues & PRs over time
- Average age trends
- SLA: items > 7 days
- Conflicts & untriaged over time

Each run stores a metric snapshot in the database for historical trending.

---

## Notifications

Priority summary sent to Discord, Slack, Teams or generic webhooks.

```bash
wshm notify
```

With `[notify] on_run = true`, `wshm run` sends it at the end of each cycle. The daemon does not send notifications: run `wshm notify` from cron for a scheduled digest.

---

## Insights, Search and Usage (Web UI)

- `/pr-insights` and `/issue-insights` — open PR / issue breakdowns and trends
- `/search` — full-text search across issues and PRs of every repo
- `/usage` — AI call counts (recorded from this release on, SQLite and PostgreSQL) and GitHub API rate limit
- `/review/radar` — PRs waiting on you (approved, or changed since you asked for changes)

---

## Revert

Undo all wshm actions on a repo.

```bash
# Preview what would be reverted
wshm revert

# Execute revert
wshm revert --apply
```

**Removes:**
- All wshm comments from issues/PRs
- All wshm-applied labels
- All triage results and PR analyses from the local database

**Web UI:** `/revert` tab — preview with counts before executing.

---

# Enterprise (self-hosted) features

These ship in the `wshm-pro` binary. Contact us at [contact@wshm.dev](mailto:contact@wshm.dev).

Only three of them check the license at runtime: `vault()` placeholders, exports and hosted diagram pages. The others are available in the `wshm-pro` binary without a license check.

## Users & RBAC

The open-source edition already has local users, the four roles enforced server-side and the `/api/v1/users` API (SQLite user store). The Enterprise edition adds the **user management page** (`/users`) and a **PostgreSQL user store** shared by every daemon replica. See [SSO & RBAC](sso-rbac.md).

## PostgreSQL Multi-Pod

Set `DATABASE_URL` to run several stateless daemon replicas on one PostgreSQL database (repository data, tracked-repo registry, settings, users, secrets and license are stored there). The open-source edition uses SQLite.

## Vault / Secrets Storage

- Encrypted secret store in PostgreSQL (AES-256-GCM, master key from `WSHM_MASTER_KEY`). The open-source edition has the same store on SQLite.
- `vault()` placeholders resolved from HashiCorp Vault, AWS Secrets Manager or GCP Secret Manager (license feature `vault`). See [Vault](vault.md).

## Export Sinks

Send pipeline events to external systems (license feature `exports`).

**Object storage:** S3 (and S3-compatible), GCS, Azure Blob
**Databases:** Elasticsearch, OpenSearch, MongoDB, MySQL

Generic HTTP webhooks with HMAC signing are open source. See [Configuration](configuration.md) for `[export]` options.

---

## Hosted PR Diagrams

License feature `diagram_hosting`.


With `[pr.review_diagrams.publish] target = "wshm"` (license feature
`diagram_hosting`), the daemon serves the interactive architecture page of
each PR at `<public_url>/diagrams/<owner>/<repo>/<pr>?k=<token>`. Only the
graph is kept in the database: the page is rendered on request, with the
diff fetched from the forge (small in-memory cache) and embedded up to
`max_diff_kb`. Responses carry an ETag and a short `Cache-Control`, and a strict CSP: the viewer's JS/CSS are vendored in the binary and served from `/diagrams/_assets/` (no CDN, no inline script, no web font). Files published to other targets inline the same assets. The
token is random per PR and is the only credential on `/diagrams/` (the
path can be exposed without the SSO proxy; proxy identity headers are
ignored there). Pages of private repos are linked under
`/api/v1/diagrams/...`, behind the dashboard authentication. Pages are
removed when the PR is closed or merged, after `retention_days` without
update, or beyond `max_pages`. See `docs/configuration.md`.

## Support

Contact us at [contact@wshm.dev](mailto:contact@wshm.dev).
