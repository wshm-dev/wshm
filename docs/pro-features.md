# Pro Features

All Pro features require a valid license. See [License & Activation](license.md).

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
wshm conflicts scan

# Attempt AI resolution
wshm conflicts scan --apply
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

## Export Sinks

Send pipeline events to external systems.

**Cloud storage:** S3, Azure Blob, GCS
**Databases:** Elasticsearch, OpenSearch, PostgreSQL, MongoDB, MySQL
**Webhooks:** Any HTTP endpoint with HMAC signing

See [Configuration](configuration.md) for `[export]` options.

---

## Vault Integration

Resolve secrets from enterprise secret managers. See [Vault](vault.md).

---

## SSO & RBAC

Single Sign-On, organizations, and role-based access control. See [SSO & RBAC](sso-rbac.md).

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

## Daemon Webhook Mode

Run wshm as a persistent service that reacts to GitHub webhooks in real-time.

```bash
wshm daemon
```

See [Daemon](daemon.md) for setup.
