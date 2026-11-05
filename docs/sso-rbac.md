# SSO & RBAC

## SSO via your auth proxy (OIDC) — open source

wshm does not embed an OIDC client. Single sign-on works by putting the daemon behind an auth proxy that handles the OIDC login (for example oauth2-proxy or Cloudflare Access) and forwards the user identity in request headers.

Enable it on the daemon:

```bash
export WSHM_TRUST_PROXY_AUTH=1
export WSHM_PROXY_AUTH_TOKEN=<shared secret>   # the proxy adds it as X-Wshm-Proxy-Token
export WSHM_ADMIN_EMAIL=admin@example.com      # first login with this email becomes admin
```

- The daemon reads the identity from `X-Auth-Request-Email`, `X-Forwarded-Email` or `X-Forwarded-User`.
- Without `WSHM_PROXY_AUTH_TOKEN`, forwarded identities are rejected (fail closed). Set `WSHM_TRUST_PROXY_AUTH_NO_TOKEN=1` only when the listener is network-isolated so that only the proxy can reach it.
- Restrict network access to the daemon port so clients cannot bypass the proxy (on Kubernetes, use a NetworkPolicy).
- Users seen for the first time through the proxy are created automatically.

## Roles

The web UI and API use four roles. Each role includes the rights of the roles below it.

| Role | Typical rights |
|------|----------------|
| `viewer` | Read dashboards, issues, PRs, reviews, insights |
| `member` | Run manual triage, analysis and incremental sync, open the Actions page |
| `operator` | Full sync, merge/close, revert, backups |
| `admin` | Settings, repositories, secrets, users, license activation, AI providers |

The backend enforces the roles. The web UI hides pages and buttons the current role cannot use.

## Enterprise (self-hosted)

The open-source edition ships the user store (SQLite), the `/api/v1/users` API and the roles above, enforced server-side.

The Enterprise edition (`wshm-pro`) adds:

- the **user management page** (`/users`: create users, change roles, delete users);
- a **PostgreSQL user store** shared by every daemon replica (`DATABASE_URL`);
- **support**.

Contact us at [contact@wshm.dev](mailto:contact@wshm.dev).
