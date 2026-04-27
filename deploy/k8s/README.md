# Raw Kubernetes manifests

Apply order:

```bash
kubectl apply -f configmap.yaml          # creates namespace + config
# Provision wshm-secrets out-of-band (external-secrets / sealed-secrets / KMS).
# For a quick test only, edit secret.example.yaml then:
#   kubectl apply -f secret.example.yaml
kubectl apply -f cronjob.yaml            # CronJob + ServiceAccount + PVC
```

Trigger an ad-hoc run from the CronJob:

```bash
kubectl -n wshm create job --from=cronjob/wshm wshm-manual-$(date +%s)
kubectl -n wshm logs -l app.kubernetes.io/name=wshm --tail=200 -f
```

Run a one-shot sync (e.g. on first install):

```bash
kubectl apply -f job.yaml
```

## Files

| File | Purpose |
|------|---------|
| `configmap.yaml` | Namespace + `wshm-config` ConfigMap (config.toml) |
| `secret.example.yaml` | Template for `wshm-secrets` — **do not commit real values** |
| `cronjob.yaml` | CronJob + ServiceAccount + PVC for periodic `wshm run --apply` |
| `job.yaml` | One-shot Job (initial sync / smoke test) |

These are minimal, opinionated manifests. For parameterized installs use
`deploy/helm/wshm/` (Helm) or `deploy/kustomize/` (Kustomize) instead.
