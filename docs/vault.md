# Vault Integration

wshm Pro supports 4 secret management providers to keep sensitive values (license keys, webhook secrets, database URIs, API keys) out of config files.

## How It Works

Any config value can use a `vault()` placeholder:

```toml
[license]
key = "vault(secret/wshm/license-key)"

[[export.webhooks]]
secret = "vault(secret/wshm/webhook-hmac)"

[export.database]
uri = "vault(secret/wshm/elastic-uri)"
```

At startup, wshm resolves all `vault()` placeholders through the configured provider before using the values.

## Providers

### HashiCorp Vault

```toml
[vault]
provider = "hashicorp"
address = "https://vault.company.com"
mount = "secret"    # KV v2 mount path
```

**Authentication** (via environment variables):

| Method | Variables |
|--------|-----------|
| Token | `VAULT_TOKEN=hvs.xxxxx` |
| AppRole | `VAULT_ROLE_ID=xxx` + `VAULT_SECRET_ID=xxx` |
| Kubernetes | `VAULT_ROLE=wshm` (auto via service account) |

**Path format:** `vault(mount/path/key)` — e.g. `vault(secret/wshm/license-key)`

**Setup:**
```bash
# Store a secret
vault kv put secret/wshm license-key=WSHM-XXXX-XXXX-XXXX

# Verify
vault kv get secret/wshm
```

**Compile flag:** `--features vault-hashicorp`

---

### AWS Secrets Manager

```toml
[vault]
provider = "aws"
```

**Authentication:**

| Method | Variables |
|--------|-----------|
| Access keys | `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` |
| Profile | `AWS_PROFILE=myprofile` |
| IAM Role | Automatic on EC2, ECS, Lambda |
| SSO | `aws sso login --profile myprofile` |

Region is auto-detected or set via `AWS_REGION`.

**Path format:** `vault(secret-name)` — the secret name in Secrets Manager.

**Setup:**
```bash
# Store a secret
aws secretsmanager create-secret \
  --name wshm/license-key \
  --secret-string "WSHM-XXXX-XXXX-XXXX"
```

**Compile flag:** `--features vault-aws`

---

### Azure Key Vault

```toml
[vault]
provider = "azure"
address = "https://wshm-vault.vault.azure.net"
```

**Authentication:**

| Method | Variables |
|--------|-----------|
| Service Principal | `AZURE_CLIENT_ID` + `AZURE_CLIENT_SECRET` + `AZURE_TENANT_ID` |
| Managed Identity | Automatic on Azure VM, AKS, App Service |
| CLI | `az login` (local dev) |

**Path format:** `vault(vault-name/secret-name)` or just `vault(secret-name)` when address is set.

**Setup:**
```bash
# Store a secret
az keyvault secret set \
  --vault-name wshm-vault \
  --name license-key \
  --value "WSHM-XXXX-XXXX-XXXX"
```

**Compile flag:** `--features vault-azure`

---

### GCP Secret Manager

```toml
[vault]
provider = "gcp"
```

**Authentication:**

| Method | Variables |
|--------|-----------|
| Service Account | `GOOGLE_APPLICATION_CREDENTIALS=/path/to/key.json` |
| Default Credentials | Automatic on GCE, GKE, Cloud Run |
| CLI | `gcloud auth application-default login` (local dev) |

**Path format:** `vault(projects/PROJECT_ID/secrets/SECRET_NAME/versions/latest)`

**Setup:**
```bash
# Store a secret
echo -n "WSHM-XXXX-XXXX-XXXX" | \
  gcloud secrets create wshm-license --data-file=-

# Use as
# vault(projects/my-project/secrets/wshm-license/versions/latest)
```

**Compile flag:** `--features vault-gcp`

---

## Security Best Practices

1. **Never commit secrets** — use vault placeholders in committed config, auth via env/IAM
2. **Least privilege** — vault tokens/roles should only access the secrets wshm needs
3. **Rotate regularly** — use short-lived tokens where possible (AppRole, Managed Identity)
4. **Audit** — enable vault audit logging to track secret access
5. **Compile only what you need** — only enable the vault feature flag for your provider

## Kubernetes Example

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: wshm
spec:
  serviceAccountName: wshm-sa  # bound to Vault role
  containers:
  - name: wshm
    image: ghcr.io/wshm-dev/wshm-pro:latest
    env:
    - name: VAULT_ADDR
      value: "https://vault.company.com"
    - name: VAULT_ROLE
      value: "wshm"
    volumeMounts:
    - name: config
      mountPath: /repo/.wshm
  volumes:
  - name: config
    configMap:
      name: wshm-config  # contains config.toml with vault() placeholders
```

## Docker Example

```bash
docker run \
  -e VAULT_TOKEN=hvs.xxxxx \
  -v $(pwd)/.wshm:/repo/.wshm \
  ghcr.io/wshm-dev/wshm-pro:latest run --apply
```

## CI/CD Example (GitHub Actions)

```yaml
- name: Run wshm
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
    ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
    WSHM_LICENSE_KEY: ${{ secrets.WSHM_LICENSE_KEY }}
  run: wshm run --apply
```

No vault needed in CI — use `WSHM_LICENSE_KEY` env var directly from GitHub Secrets.
