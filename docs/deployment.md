# Deployment

How wshm is packaged for the various places it can run, and where each method lives.

## Public images

Multi-arch images (`linux/amd64` + `linux/arm64`) are published to Docker Hub on every release tag:

| Image | Registry | Pull |
|-------|----------|------|
| `wshm` (OSS) | Docker Hub | `docker pull innovtech/wshm:latest` |
| `wshm-pro` | Docker Hub | `docker pull innovtech/wshm-pro:latest` |

Docker Hub is the only registry — GHCR is not used.

## Layout

All deployment artifacts live under `deploy/` in this repo, one subdirectory per method. The Enterprise equivalents live in `wshm-pro/deploy/`.

```
deploy/
  helm/wshm/      # Helm chart for Kubernetes (Job/CronJob)
  kustomize/      # Kustomize base + prod/staging overlays
  k8s/            # Raw manifests (CronJob, Job, ConfigMap, Secret example)
  compose/        # docker-compose.yml for single-host        (planned)
  ansible/        # Ansible role for VM/bare-metal            (planned)
  cloud-init/     # cloud-init snippets for cloud VMs         (planned)
```

## Status

| Method        | Target                           | Status   |
|---------------|----------------------------------|----------|
| Binary        | Linux/macOS/Windows              | Shipped  |
| `.deb`        | Debian/Ubuntu                    | Shipped  |
| Homebrew      | macOS, Linux                     | Shipped  |
| Docker        | Any Docker host                  | Shipped  |
| Helm          | Kubernetes (`Job`/`CronJob`)     | Shipped  |
| Kustomize     | Kubernetes (base + prod/staging) | Shipped  |
| Raw manifests | Kubernetes                       | Shipped  |
| docker-compose| Single-host self-hosted          | Planned  |
| Ansible role  | VM / bare-metal                  | Planned  |
| cloud-init    | AWS/GCP/Azure/Hetzner VMs        | Planned  |
| GitHub Action | CI usage                         | Planned ([#22](https://github.com/wshm-dev/wshm/issues/22)) |

## Why a single repo (for now)

Deployment artifacts stay co-located with the code they deploy until two conditions are met:

1. The chart/role surface becomes large enough to warrant its own release cadence (versioned `values.yaml`, dependencies, multi-environment overlays).
2. Outside contributors start submitting packaging changes that don't belong in the core repo's review cycle.

When both happen, the `deploy/` tree gets extracted into a dedicated public repo (`wshm-deploy` or `wshm-charts`) and a Helm chart is published to a registry. Until then, a single source of truth keeps versions of the binary and the manifests in lockstep — every release tag rebuilds both.

## Open source vs. Enterprise

`wshm` (OSS) ships the CLI, TUI, daemon, and embedded web dashboard. Use `Job`/`CronJob` for periodic triage runs, or `Deployment` + `Service` + `Ingress` to expose the daemon's web UI long-running. Review, auto-fix, conflict detection, changelog and reports are part of it. The Enterprise edition (`wshm-pro`) adds the user management page, PostgreSQL multi-pod (storage, users, secrets), vault placeholders, exports, hosted diagram pages and support on top of the same daemon.
