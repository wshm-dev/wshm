# CLI Reference

## Global Flags

| Flag | Description |
|------|-------------|
| `--offline` | Skip GitHub sync, use cached data only |
| `--verbose` / `-v` | Detailed output |
| `--json` | JSON output for scripting |
| `--csv` | CSV output |
| `--repo <owner/repo>` | Override detected repo |

Commands that change something on the forge are dry-run by default and take `--apply`.

## Commands

All commands below are open source. No license key is needed.

### Triage, PRs and queue

```
wshm                             Show status (from cache, instant)
wshm sync                        Force full sync from GitHub
wshm triage [--issue <N>]        Classify issues [or single issue]
wshm triage --apply              Classify + label + comment
wshm triage --retriage           Re-evaluate stale triage results
wshm pr [--pr <N>]               Analyze PRs [or single PR]
wshm pr --apply                  Analyze + label + comment
wshm queue                       Show ranked merge queue
wshm queue --apply               Merge top PR if above threshold
wshm health                      PR health: duplicates, stale/zombie PRs
wshm run                         Full cycle: sync + triage + analyze + queue + conflict detection
wshm run --apply                 Full cycle with actions
```

### Review, fix and improve

```
wshm review --pr <N>             Inline code review on PR diff
wshm review --pr <N> --apply     Review + post comments to GitHub
wshm fix --issue <N>             Auto-generate fix for issue
wshm fix --issue <N> --apply     Generate fix + open draft PR
wshm conflicts                   Detect conflicting PRs (read-only)
wshm improve                     Analyze codebase, propose improvements
wshm improve --apply             Propose + create GitHub issues
```

### Reporting

```
wshm changelog --days <N>        Generate changelog from merged PRs
wshm changelog --format json     JSON format (default: md)
wshm report --format html        Health report (md, html, pdf)
wshm dashboard                   Export time-series metrics dashboard (HTML)
wshm plan [--limit <N>]          Prioritized action plan across monitored repos
wshm notify                      Send priority summary to Discord/Slack/Teams/webhook
wshm summary                     Show daily digest summary
wshm context                     Export repo context as LLM-ready markdown
```

### Daemon, data and setup

```
wshm daemon                      Run webhook server + polling daemon + web UI
wshm tui                         Interactive terminal dashboard
wshm revert                      Preview what would be reverted
wshm revert --apply              Remove all wshm actions from GitHub
wshm backup                      Snapshot .wshm/ data
wshm restore <file>              Restore from a backup file
wshm config init                 Create .wshm/config.toml template
wshm login                       Authenticate with GitHub / AI provider
wshm login --license             Activate an Enterprise license
wshm update                      Check/install latest release
wshm telemetry                   Manage anonymous telemetry consent
```

## Examples

```bash
# Triage all open issues, apply labels
wshm triage --apply

# Analyze a specific PR
wshm pr --pr 42

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

# Run the daemon
wshm daemon
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Error |
