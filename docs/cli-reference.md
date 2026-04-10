# CLI Reference

## Global Flags

| Flag | Description |
|------|-------------|
| `--apply` | Actually perform actions (dry-run by default) |
| `--offline` | Skip GitHub sync, use cached data only |
| `--verbose` / `-v` | Detailed output |
| `--json` | JSON output for scripting |
| `--csv` | CSV output |
| `--repo <owner/repo>` | Override detected repo |

## Commands

### Core (Free)

```
wshm                             Show status (from cache, instant)
wshm sync                        Force full sync from GitHub
wshm triage [--issue <N>]        Classify issues [or single issue]
wshm triage --apply              Classify + label + comment
wshm triage --retriage           Re-evaluate stale triage results
wshm pr analyze [--pr <N>]       Analyze PRs [or single PR]
wshm pr analyze --apply          Analyze + label + comment
wshm queue                       Show ranked merge queue
wshm queue --apply               Merge top PR if above threshold
wshm conflicts scan              Detect conflicting PRs
wshm run                         Full cycle: sync + triage + analyze + queue + conflicts
wshm run --apply                 Full cycle with actions
wshm notify                      Send priority summary to Discord/Slack/Teams/webhook
wshm config init                 Create .wshm/config.toml template
wshm login                       Authenticate with GitHub / AI provider
wshm login --license             Activate Pro license
wshm tui                         Interactive terminal dashboard
```

### Pro

```
wshm review --pr <N>             Inline code review on PR diff
wshm review --pr <N> --apply     Review + post comments to GitHub
wshm fix --issue <N>             Auto-generate fix for issue
wshm fix --issue <N> --apply     Generate fix + open draft PR
wshm improve                     Analyze codebase, propose improvements
wshm improve --apply             Propose + create GitHub issues
wshm changelog --days <N>        Generate changelog from merged PRs
wshm changelog --format markdown Markdown format (default)
wshm changelog --format json     JSON format
wshm report --format json        Generate health report (JSON)
wshm report --format html        Interactive HTML report with charts
wshm dashboard                   Export time-series metrics dashboard
wshm revert                      Preview what would be reverted
wshm revert --apply              Remove all wshm actions from GitHub
wshm conflicts scan --apply      Detect + attempt AI resolution
wshm context                     Export repo context as LLM-ready markdown
```

### Daemon & Infra

```
wshm daemon                      Run webhook server + polling daemon
wshm update                      Check/install latest release
wshm model pull <name>           Pull local AI model
wshm model list                  List downloaded models
wshm model remove <name>         Remove a model
wshm migrate                     Migrate SQLite to PostgreSQL
```

## Examples

```bash
# Triage all open issues, apply labels
wshm triage --apply

# Analyze a specific PR
wshm pr analyze --pr 42

# Review PR and post comments
wshm review --pr 42 --apply

# Generate a fix for issue #15
wshm fix --issue 15 --apply

# Full cycle in offline mode (use cache)
wshm run --offline

# Export status as JSON
wshm --json

# Generate last 30 days changelog
wshm changelog --days 30

# Run the daemon with polling every 60s
wshm daemon
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | General error |
| 2 | Configuration error |
| 3 | Authentication error (no token) |
| 4 | License error (Pro feature without license) |
