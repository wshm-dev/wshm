# Daemon & Web UI

Run wshm as a persistent background service with webhook support and an embedded web dashboard.

## Quick Start

```bash
wshm daemon
```

Opens `http://127.0.0.1:3000` with the web dashboard.

## Modes

### Polling Mode (default)

Periodically syncs with GitHub and runs pipelines.

```toml
# ~/.wshm/global.toml
[daemon]
bind = "127.0.0.1:3000"
poll = true
poll_interval = 30    # seconds between polls
apply = false         # dry-run by default
```

No public IP needed. Works behind firewalls.

### Webhook Mode (Pro)

Reacts to GitHub events in real-time.

```toml
[daemon]
bind = "0.0.0.0:3000"
webhook_secret = "your-github-webhook-secret"
apply = true
```

**GitHub Webhook Setup:**
1. Repo Settings > Webhooks > Add webhook
2. Payload URL: `https://your-server.com/webhook`
3. Content type: `application/json`
4. Secret: same as `webhook_secret`
5. Events: Issues, Pull requests, Issue comments

## Multi-Repo

Manage multiple repos from a single daemon instance.

```toml
# ~/.wshm/global.toml
[daemon]
bind = "127.0.0.1:3000"

[[repos]]
slug = "acme/frontend"
path = "/home/user/repos/frontend"
enabled = true
apply = true

[[repos]]
slug = "acme/backend"
path = "/home/user/repos/backend"
enabled = true
apply = false

[[repos]]
slug = "acme/docs"
path = "/home/user/repos/docs"
enabled = false    # skip this repo
```

## Web Dashboard

The daemon embeds a full SvelteKit web UI:

| Route | Description |
|-------|-------------|
| `/` | Dashboard — overview stats |
| `/issues` | Open issues with triage results |
| `/prs` | Pull requests with risk/CI status |
| `/triage` | Recent triage activity |
| `/queue` | Merge queue with scores |
| `/review` | Code reviews (Pro) |
| `/conflicts` | Conflict scan (Pro) |
| `/autofix` | Auto-fix candidates (Pro) |
| `/improve` | Improvement suggestions (Pro) |
| `/changelog` | Auto-generated changelog (Pro) |
| `/reports` | Repo health metrics (Pro) |
| `/revert` | Revert preview (Pro) |
| `/activity` | Recent activity feed |
| `/actions` | Priority action items |
| `/settings` | License, color scheme, config |

### Authentication

```toml
[web]
username = "admin"
password = "changeme"
```

Uses HTTP Basic Auth. If no password is set, the dashboard is open (suitable for localhost only).

## Systemd Service

```ini
# /etc/systemd/system/wshm.service
[Unit]
Description=wshm daemon
After=network.target

[Service]
Type=simple
User=wshm
WorkingDirectory=/home/wshm
ExecStart=/usr/local/bin/wshm daemon
Restart=always
RestartSec=5

Environment=GITHUB_TOKEN=ghp_xxxxx
Environment=ANTHROPIC_API_KEY=sk-ant-xxxxx
Environment=WSHM_LICENSE_KEY=WSHM-XXXX-XXXX-XXXX

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now wshm
sudo journalctl -u wshm -f
```

## Docker

```bash
docker run -d \
  --name wshm \
  -p 3000:3000 \
  -e GITHUB_TOKEN=ghp_xxxxx \
  -e ANTHROPIC_API_KEY=sk-ant-xxxxx \
  -e WSHM_LICENSE_KEY=WSHM-XXXX-XXXX-XXXX \
  -v /home/user/repos:/repos \
  -v /home/user/.wshm:/root/.wshm \
  ghcr.io/wshm-dev/wshm-pro:latest daemon
```

## API Endpoints

All endpoints require Basic Auth (when password is configured).

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/status` | Aggregate status across repos |
| GET | `/api/v1/issues` | Open issues with triage data |
| GET | `/api/v1/pulls` | Open PRs with analysis data |
| GET | `/api/v1/triage` | Recent triage results |
| GET | `/api/v1/queue` | Merge queue with scores |
| GET | `/api/v1/activity` | Combined activity feed |
| GET | `/api/v1/reviews` | Code review data (Pro) |
| GET | `/api/v1/conflicts` | Conflict scan (Pro) |
| GET | `/api/v1/improvements` | Improvement suggestions (Pro) |
| GET | `/api/v1/changelog` | Changelog from closed PRs (Pro) |
| GET | `/api/v1/reports` | Repo health metrics (Pro) |
| GET | `/api/v1/revert/preview` | Revert action preview (Pro) |
| GET | `/api/v1/autofix/candidates` | Auto-fix candidates (Pro) |
| GET | `/api/v1/license` | License status + features |
| POST | `/api/v1/license/activate` | Activate a license key |
| POST | `/webhook` | GitHub webhook receiver |
| GET | `/health` | Health check (no auth) |

All GET endpoints accept an optional `?repo=owner/name` query parameter to filter by repo.
