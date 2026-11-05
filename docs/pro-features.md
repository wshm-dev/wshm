# Open source vs Enterprise

wshm follows an open-core model:

- **Open source** (`wshm`, Apache-2.0): every feature on this page except the Enterprise section. No license key needed.
- **Enterprise (self-hosted)** (`wshm-pro`): everything in open source, plus users & RBAC, audit log, PostgreSQL multi-pod, vault/secrets storage, exports and support. Contact us at [contact@wshm.dev](mailto:contact@wshm.dev).

The commands below use the `wshm` binary name. The Enterprise binary is `wshm-pro` and accepts the same commands (the Docker image installs it as `wshm`).

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

Automatically generate fix PRs from issue descriptions.

```bash
# Generate a fix for an issue
wshm fix --issue 15 --apply
```

**What it does:**
1. Reads the issue description from the cache
2. Uses AI (Claude Code, Codex, or Podman container) to generate a fix
3. Runs security scan on the diff (17 patterns: curl, eval, API keys, SSH keys...)
4. Optionally runs tests with configurable retries
5. Creates a draft PR on branch `wshm/fix-<issue_number>`
6. Comments on the issue with a link to the PR

**Eligible issues:** bugs with `is_simple_fix = true` and confidence >= 85%.

**Web UI:** `/autofix` tab — shows eligible candidates.

**Config:**
```toml
[triage]
auto_fix = true
auto_fix_confidence = 0.85

[fix]
test_command = "cargo test"
test_retries = 2
trusted_authors_only = false
```

---

## Conflict Resolution

Detect and resolve merge conflicts across open PRs.

```bash
# Scan for conflicts (dry run)
wshm conflicts

# Attempt AI resolution
wshm conflicts --apply
```

**What it does:**
- Scans all open PRs for `mergeable = false`
- Groups PRs by clean / conflicting / unknown status
- (With `--apply`) Attempts AI-powered rebase resolution
- Never force-pushes — creates new commits

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

With `[notify] on_run = true`, `wshm run` sends it at the end of each cycle.

---

## Insights, Search and Usage (Web UI)

- `/pr-insights` and `/issue-insights` — open PR / issue breakdowns and trends
- `/search` — full-text search across issues and PRs of every repo
- `/usage` — AI call counts and GitHub API rate limit
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

These need a license. Contact us at [contact@wshm.dev](mailto:contact@wshm.dev).

## Users & RBAC

User management page in the web UI and a PostgreSQL-backed user store shared by every daemon replica. SSO via your auth proxy (OIDC) and the role model are open source. See [SSO & RBAC](sso-rbac.md).

## Audit Log

Trail of actions taken in wshm.

## PostgreSQL Multi-Pod

Set `DATABASE_URL` to run several stateless daemon replicas on one PostgreSQL database (repositories, settings, users, secrets and license are stored there). The open-source edition uses SQLite.

## Vault / Secrets Storage

Encrypted secret store in PostgreSQL (AES-256-GCM, master key from `WSHM_MASTER_KEY`) and `vault()` placeholders resolved from HashiCorp Vault, AWS Secrets Manager or GCP Secret Manager. See [Vault](vault.md).

## Export Sinks

Send pipeline events to external systems.

**Object storage:** S3 (and S3-compatible), GCS, Azure Blob
**Databases:** Elasticsearch, OpenSearch, MongoDB, MySQL

Generic HTTP webhooks with HMAC signing are open source. See [Configuration](configuration.md) for `[export]` options.

---

## Hosted PR Diagrams

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
