# SSO & RBAC

Pro feature. Manage organizations, members, roles, and enterprise SSO.

## Organizations

Organizations group users and repos under a shared license and permissions model.

### Create an Organization

Via the portal (app.wshm.dev) or API:

```bash
curl -X POST https://api.wshm.dev/api/v1/orgs \
  -H 'Content-Type: application/json' \
  -d '{"token": "YOUR_SESSION_TOKEN", "name": "Acme Corp", "slug": "acme"}'
```

The creator becomes the **owner**.

### Invite Members

```bash
curl -X POST https://api.wshm.dev/api/v1/orgs/ORG_ID/invite \
  -H 'Content-Type: application/json' \
  -d '{"token": "...", "email": "dev@acme.com", "role": "member"}'
```

The invited user receives a token to accept the invitation.

## Roles & Permissions

4 roles with graduated permissions:

| Permission | Owner | Admin | Member | Viewer |
|------------|:-----:|:-----:|:------:|:------:|
| **Organization** |
| View org | x | x | x | x |
| Update org settings | x | x | | |
| Delete org | x | | | |
| Transfer ownership | x | | | |
| **Members** |
| View members | x | x | x | x |
| Invite members | x | x | | |
| Remove members | x | x | | |
| Change roles | x | x | | |
| **Repos** |
| View repo data | x | x | x | x |
| Write (triage, label, merge) | x | x | x | |
| **Pipelines** |
| View triage/review/queue | x | x | x | x |
| Apply triage | x | x | x | |
| Apply review (post comments) | x | x | x | |
| Apply merge | x | x | x | |
| Apply auto-fix | x | x | x | |
| Revert actions | x | x | x | |
| **Security** |
| View SSO config | x | x | | |
| Configure SSO | x | x | | |
| View audit log | x | x | | |
| Manage licenses | x | x | | |

## SSO — SAML 2.0

### Configure SAML

```bash
curl -X POST https://api.wshm.dev/api/v1/orgs/ORG_ID/sso/saml \
  -H 'Content-Type: application/json' \
  -d '{
    "token": "...",
    "entity_id": "https://idp.acme.com/saml",
    "sso_url": "https://idp.acme.com/saml/sso",
    "certificate": "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----",
    "auto_provision": true,
    "default_role": "member",
    "allowed_domains": "acme.com",
    "enforce_sso": false
  }'
```

### SP Metadata

Your IdP needs the SP metadata URL:

```
https://api.wshm.dev/api/v1/sso/YOUR_ORG_SLUG/saml/metadata
```

### Login URL

Members use:

```
https://api.wshm.dev/api/v1/sso/YOUR_ORG_SLUG/saml/login
```

### How It Works

1. User visits the SAML login URL
2. wshm redirects to your IdP with a SAML AuthnRequest
3. User authenticates with the IdP
4. IdP POSTs the SAML assertion to the ACS URL
5. wshm extracts the email from the assertion
6. If `auto_provision` is on, user is created automatically
7. User is added to the org with `default_role`
8. Session is created and user is redirected to the portal

### IdP Setup (Okta, Azure AD, Google Workspace)

**Okta:**
1. Create a SAML 2.0 app
2. Set SSO URL to: `https://api.wshm.dev/api/v1/sso/YOUR_SLUG/saml/acs`
3. Set Entity ID to: `https://api.wshm.dev/api/v1/sso/YOUR_SLUG/saml/metadata`
4. Attribute: `emailAddress` -> `user.email`

**Azure AD:**
1. Enterprise Applications > New > Non-gallery
2. Single sign-on > SAML
3. Same URLs as Okta
4. Identifier: the Entity ID
5. Reply URL: the ACS URL

**Google Workspace:**
1. Admin console > Apps > Web and mobile apps > Add SAML app
2. ACS URL and Entity ID as above
3. NameID: Email

## SSO — OIDC

### Configure OIDC

```bash
curl -X POST https://api.wshm.dev/api/v1/orgs/ORG_ID/sso/oidc \
  -H 'Content-Type: application/json' \
  -d '{
    "token": "...",
    "issuer": "https://accounts.google.com",
    "client_id": "xxxxx.apps.googleusercontent.com",
    "client_secret": "GOCSPX-xxxxx",
    "scopes": "openid email profile",
    "auto_provision": true,
    "default_role": "member",
    "allowed_domains": "acme.com"
  }'
```

### Login URL

```
https://api.wshm.dev/api/v1/sso/YOUR_ORG_SLUG/oidc/login
```

### Supported OIDC Providers

Any OIDC-compliant provider:
- Google Workspace
- Azure AD / Entra ID
- Okta
- Auth0
- Keycloak
- GitLab (self-managed)

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `auto_provision` | `true` | Create users on first SSO login |
| `default_role` | `member` | Role assigned to auto-provisioned users |
| `allowed_domains` | (none) | Comma-separated email domain whitelist |
| `enforce_sso` | `false` | Require SSO for all org members |

## Audit Log

All RBAC actions are logged:

```bash
curl https://api.wshm.dev/api/v1/orgs/ORG_ID/audit?token=...
```

Returns:
```json
{
  "entries": [
    {
      "action": "member.invited",
      "user_id": "...",
      "target_type": "invitation",
      "details": {"email": "dev@acme.com", "role": "member"},
      "created_at": "2026-04-07T..."
    }
  ]
}
```

Tracked actions: `org.created`, `member.invited`, `member.joined`, `member.role_changed`, `member.removed`, `sso.configured`, `sso.disabled`.

## Disable SSO

```bash
curl -X DELETE https://api.wshm.dev/api/v1/orgs/ORG_ID/sso/saml \
  -H 'Content-Type: application/json' \
  -d '{"token": "..."}'
```
