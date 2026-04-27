# Deployment artifacts

This directory holds packaging for the various places `wshm` (OSS) can run.
The persistent daemon + Web UI is a Pro feature and lives in the `wshm-pro`
repo, so the OSS chart focuses on `Job` / `CronJob` patterns suitable for
periodic triage.

## Layout

| Path | Method | Use when |
|------|--------|---------|
| [`helm/wshm/`](helm/wshm/) | Helm chart | Templated install with values overrides; multi-env via separate releases |
| [`kustomize/`](kustomize/) | Kustomize base + `prod`/`staging` overlays | GitOps with environment overlays |
| [`k8s/`](k8s/) | Raw manifests | Quickest path to a working CronJob; no extra tools |

All three deploy the same shape: `CronJob` (default `*/30 * * * *`) running
`wshm run --apply` against a config mounted from a `ConfigMap`, with
credentials sourced from a `Secret`, and `.wshm/state.db` cached on a `PVC`.

## Common shape

- Image: `innovtech/wshm:<appVersion>` (multi-arch `linux/amd64` + `linux/arm64`)
- Working directory: `/data` (PVC, holds `.wshm/state.db` between runs)
- Config: `/etc/wshm/config.toml` (ConfigMap, optional in Helm)
- Secret keys read at runtime: `GITHUB_TOKEN` (required), `ANTHROPIC_API_KEY` (or other provider), `WSHM_LICENSE_KEY` (Pro only)
- Pod runs non-root (`65532`), read-only rootfs, all caps dropped

## Choosing between the three

- **Just want to try it on a cluster?** Use `k8s/` — three `kubectl apply` calls.
- **Multiple environments, GitOps?** Use `kustomize/` — base + overlays, no templating.
- **Configurable install for downstream users?** Use `helm/wshm/` — `values.yaml` exposes everything.

## Secrets

None of the artifacts ship real credentials. Provision `wshm-secrets`
out-of-band using whichever pattern your cluster already uses:
[external-secrets-operator](https://external-secrets.io/),
[sealed-secrets](https://github.com/bitnami-labs/sealed-secrets), or your
cloud KMS. The Helm chart can also materialise a Secret from values
(`secrets.create=true`) — convenient for local testing, **never** for prod.

## Pro

For the persistent daemon (`Deployment` + `Service` + `Ingress`, Web UI,
webhook server, multi-repo runtime), use the `wshm-pro` chart.
