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

[triage]
enabled = true
auto_fix = false                  # attempt auto-fix for simple bugs (Pro)
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

[queue]
enabled = true
merge_threshold = 15              # minimum score to auto-merge
strategy = "rebase"               # merge, rebase, squash

[conflicts]
enabled = true
auto_resolve = false              # AI conflict resolution (Pro)
auto_resolve_confidence = 0.85

[fix]
# auto_fix_confidence = 0.85
# trusted_authors_only = false
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
# on_run = true
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
# [export.storage]
# provider = "s3"                 # s3, azure, gcs (Pro)
# bucket = "wshm-events"
# region = "eu-west-1"
#
# [export.database]
# provider = "elastic"            # elastic, opensearch, postgres, mongodb, mysql (Pro)
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

## Environment Variables

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` | GitHub personal access token |
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key (if using OpenAI) |
| `WSHM_LICENSE_KEY` | License key (alternative to vault/config) |
| `WSHM_TOKEN` | Alternative to GITHUB_TOKEN |
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
