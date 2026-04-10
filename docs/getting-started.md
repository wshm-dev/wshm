# Getting Started

## Installation

### Binary (recommended)

```bash
# macOS / Linux
curl -fsSL https://wshm.dev/install.sh | sh

# Or with cargo
cargo install wshm-pro
```

### Docker

```bash
docker run -v $(pwd):/repo ghcr.io/wshm-dev/wshm-pro:latest triage
```

### GitHub Action

```yaml
- uses: wshm-dev/wshm-action@v1
  with:
    token: ${{ secrets.GITHUB_TOKEN }}
    anthropic_key: ${{ secrets.ANTHROPIC_API_KEY }}
```

## Quick Setup

### 1. Initialize config

```bash
cd your-repo
wshm config init
```

This creates `.wshm/config.toml` with default settings.

### 2. Authenticate

```bash
# GitHub token (from env or interactive)
export GITHUB_TOKEN=ghp_xxxxx

# AI provider key
export ANTHROPIC_API_KEY=sk-ant-xxxxx

# Or login interactively
wshm login
```

### 3. First sync

```bash
wshm sync
```

This fetches all open issues and PRs into the local SQLite cache (`.wshm/state.db`).

### 4. Triage issues

```bash
# Dry run (see what would happen)
wshm triage

# Apply labels and comments
wshm triage --apply
```

### 5. Analyze PRs

```bash
wshm pr analyze --apply
```

### 6. Full cycle

```bash
wshm run --apply
```

This runs: sync -> triage -> PR analysis -> merge queue -> conflict scan.

## What's Next

- [Configuration](configuration.md) — customize behavior
- [Pro Features](pro-features.md) — unlock code review, auto-fix, and more
- [Daemon](daemon.md) — run as a background service
- [CLI Reference](cli-reference.md) — all commands
