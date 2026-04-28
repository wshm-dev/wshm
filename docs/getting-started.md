# Getting Started

## Installation

### Linux — install script (with SHA256 verification)

```bash
curl -fsSL https://raw.githubusercontent.com/wshm-dev/wshm/main/install.sh | sh
```

Re-run the same command to upgrade. Pin a specific version with:

```bash
curl -fsSL https://raw.githubusercontent.com/wshm-dev/wshm/main/install.sh | sh -s -- --version v0.28.3
```

Every download is verified against the release's `checksums.txt`.

### macOS / Linux — Homebrew

```bash
brew tap wshm-dev/tap
brew install wshm
```

### Linux — `.deb` (amd64 / arm64)

```bash
# amd64
curl -LO https://github.com/wshm-dev/wshm/releases/latest/download/wshm_$(curl -s https://api.github.com/repos/wshm-dev/wshm/releases/latest | grep tag_name | cut -d'"' -f4 | tr -d v)_amd64.deb
sudo dpkg -i wshm_*_amd64.deb
```

### Docker (Docker Hub)

```bash
docker run --rm -v $(pwd):/repo -w /repo innovtech/wshm:latest triage
```

Multi-arch image (`linux/amd64` + `linux/arm64`). Pro image: `innovtech/wshm-pro:latest`.

> A GitHub Action is planned but not yet released — track [#22](https://github.com/wshm-dev/wshm/issues/22).

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

# Or login interactively (GitHub + AI provider, including Claude Max OAuth)
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

OSS `run` does: **sync → triage → PR analysis → merge queue**. AI conflict resolution is a Pro-only stage and only runs in the Pro binary.

## What's Next

- [Configuration](configuration.md) — customize behavior
- [Pro Features](pro-features.md) — unlock code review, auto-fix, and more
- [Daemon](daemon.md) — run as a background service
- [CLI Reference](cli-reference.md) — all commands
