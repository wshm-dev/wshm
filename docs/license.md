# License & Activation

## Editions

| Edition | Features |
|---------|----------|
| **Open source** (Apache-2.0, no license key) | Triage, PR analysis, merge queue, PR health, inline code review, auto-fix, conflict resolution, improvement proposals, changelog, reports, metrics dashboard, action plan, notifications (Discord, Slack, Teams, webhooks), PR and issue insights, search, usage dashboard, daemon and web UI, TUI, SQLite storage, SSO via your auth proxy (OIDC) |
| **Enterprise (self-hosted)** | Everything in open source, plus users & RBAC, audit log, PostgreSQL multi-pod, vault/secrets storage, exports (S3, GCS, Azure Blob, MySQL, MongoDB, Elasticsearch, OpenSearch), support |

Enterprise: contact us at [contact@wshm.dev](mailto:contact@wshm.dev).

A license key is only needed for Enterprise features. Every open-source feature runs without one, in both the `wshm` and `wshm-pro` binaries.

## Activation

### Interactive (recommended)

```bash
wshm login --license
```

Enter your license key when prompted. The key is activated with the API and a JWT token is cached at `~/.wshm/license.jwt`.

### Environment Variable

```bash
export WSHM_LICENSE_KEY=WSHM-XXXX-XXXX-XXXX
wshm run --apply
```

Useful for CI/CD pipelines and containers.

### Config File

Add to `.wshm/config.toml`:

```toml
[license]
key = "vault(secret/wshm/license-key)"  # recommended: use vault
# path = "~/.wshm/license.jwt"          # or: explicit JWT file path
```

**Never put the license key in plain text in config.toml.** Use vault placeholders or environment variables.

## Resolution Order

When wshm starts, it resolves the license in this order:

1. **Vault placeholder** — if `[license] key` contains `vault(...)`, resolve via the configured vault provider
2. **Environment variable** — `WSHM_LICENSE_KEY`
3. **Config path** — `[license] path` pointing to a JWT file
4. **Default path** — `~/.wshm/license.jwt` (created by `wshm login --license`)

The first source that returns a valid value wins.

## Vault Examples

### HashiCorp Vault

```toml
[vault]
provider = "hashicorp"
address = "https://vault.company.com"
mount = "secret"

[license]
key = "vault(secret/wshm/license-key)"
```

Auth via environment:
```bash
export VAULT_TOKEN=hvs.xxxxxxxxxxxxx
# or AppRole:
export VAULT_ROLE_ID=xxxxx
export VAULT_SECRET_ID=xxxxx
```

### AWS Secrets Manager

```toml
[vault]
provider = "aws"

[license]
key = "vault(wshm/license-key)"
```

Auth via environment or IAM role:
```bash
export AWS_ACCESS_KEY_ID=AKIA...
export AWS_SECRET_ACCESS_KEY=xxxxx
export AWS_REGION=eu-west-1
# Or: IAM role on EC2/ECS (automatic)
```

### GCP Secret Manager

```toml
[vault]
provider = "gcp"

[license]
key = "vault(projects/my-project/secrets/wshm-license/versions/latest)"
```

Auth via service account or metadata:
```bash
export GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account.json
# Or: Default credentials on GCE/GKE (automatic)
```

## Machine Activation

Each license key supports up to **3 machine activations** by default. A machine ID is generated from a hash of your hostname + username.

```bash
# View your machine ID
wshm login --license
# The machine ID is displayed during activation
```

To deactivate a machine (free up a slot), contact support or use the portal.

## Checking License Status

```bash
# In the CLI
wshm triage  # shows license status in output

# In the web dashboard
# Go to Settings > License
```

## Offline Usage

Once activated, the JWT token is cached locally. wshm validates the token offline (checking expiry and signature). No network call is needed for license checks after initial activation.

The token is valid for the duration specified in your plan (typically 30 days for monthly, 365 days for annual). wshm will warn you 7 days before expiry.
