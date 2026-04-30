# Deploying wshm on Kubernetes

This guide deploys the wshm daemon on a Kubernetes cluster, exposing the
embedded web UI behind Google SSO via Traefik + oauth2-proxy.

## Prerequisites

- Kubernetes 1.20+
- Traefik with `IngressRoute` CRDs installed (or substitute your ingress
  controller's equivalent middleware)
- A Google OAuth 2.0 Client (web type) — get one from
  [Google Cloud Console](https://console.cloud.google.com/apis/credentials)
- The `wshm` Docker image built with the embedded web UI: `innovtech/wshm:latest`
  (any tag from v0.28.4+ which ships the UI bundle)

## 1. Generate the auth tokens

### Claude OAuth (Max/Pro/Team subscription)

In any terminal where you've authenticated Claude Code:

```bash
claude /token
```

Copy the printed token. wshm reads either `CLAUDE_CODE_OAUTH_TOKEN` (the
name printed by `claude /token`) or `ANTHROPIC_OAUTH_TOKEN` — both work.

If you don't have a Claude subscription, use a regular API key from
[console.anthropic.com](https://console.anthropic.com/) and set
`ANTHROPIC_API_KEY` instead.

### GitHub Personal Access Token

Create a PAT at [github.com/settings/tokens](https://github.com/settings/tokens)
with `repo` scope (and `read:org` if you target org-owned repos).

### Web admin password

Optional but recommended in production. Pick any strong secret. wshm reads
it from the `WSHM_WEB_PASSWORD` env var. If unset, wshm auto-generates one
on first start and prints it to stderr (visible via `kubectl logs`).

## 2. Create the Kubernetes Secret

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: wshm-secrets
  namespace: wshm
type: Opaque
stringData:
  WSHM_WEB_PASSWORD: "change-me-strong-password"
  GITHUB_TOKEN: "ghp_xxxxx..."
  CLAUDE_CODE_OAUTH_TOKEN: "sk-ant-oat01-..."
  # OR if you use an API key instead:
  # ANTHROPIC_API_KEY: "sk-ant-api03-..."
```

```bash
kubectl create namespace wshm
kubectl apply -f wshm-secrets.yaml
```

## 3. Deployment + Service

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: wshm
  namespace: wshm
spec:
  replicas: 1
  strategy:
    type: Recreate         # SQLite state — never two pods at once
  selector:
    matchLabels: { app: wshm }
  template:
    metadata:
      labels: { app: wshm }
    spec:
      containers:
        - name: wshm
          image: innovtech/wshm:latest
          args: ["daemon", "--repo", "your-org/your-repo", "--poll"]
          ports:
            - containerPort: 3000
          envFrom:
            - secretRef:
                name: wshm-secrets
          volumeMounts:
            - name: state
              mountPath: /home/wshm/.wshm
          readinessProbe:
            httpGet: { path: /health, port: 3000 }
            periodSeconds: 10
          livenessProbe:
            httpGet: { path: /health, port: 3000 }
            periodSeconds: 30
      volumes:
        - name: state
          persistentVolumeClaim:
            claimName: wshm-state
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: wshm-state
  namespace: wshm
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 1Gi
---
apiVersion: v1
kind: Service
metadata:
  name: wshm
  namespace: wshm
spec:
  selector: { app: wshm }
  ports:
    - port: 80
      targetPort: 3000
```

For multi-repo mode, replace `--repo your-org/your-repo` with
`--config /etc/wshm/global.toml` and mount a ConfigMap with `global.toml`
listing each `[[repos]]` entry. New repos can then be added live via the
Settings page (POST `/api/v1/repos`).

## 4. oauth2-proxy + Traefik ForwardAuth

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: oauth2-proxy
  namespace: wshm
type: Opaque
stringData:
  client-id: "xxxxx.apps.googleusercontent.com"
  client-secret: "GOCSPX-xxxxx"
  cookie-secret: "$(openssl rand -base64 32 | head -c 32)"
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: oauth2-proxy
  namespace: wshm
spec:
  replicas: 1
  selector:
    matchLabels: { app: oauth2-proxy }
  template:
    metadata:
      labels: { app: oauth2-proxy }
    spec:
      containers:
        - name: oauth2-proxy
          image: quay.io/oauth2-proxy/oauth2-proxy:latest
          args:
            - --provider=google
            - --email-domain=your-company.com   # restrict to your domain
            - --upstream=static://200
            - --http-address=0.0.0.0:4180
            - --cookie-secure=true
            - --cookie-domain=wshm.example.com
            - --whitelist-domain=wshm.example.com
            - --redirect-url=https://wshm.example.com/oauth2/callback
            - --reverse-proxy=true
            - --pass-access-token=true
            - --set-xauthrequest=true
          env:
            - name: OAUTH2_PROXY_CLIENT_ID
              valueFrom: { secretKeyRef: { name: oauth2-proxy, key: client-id } }
            - name: OAUTH2_PROXY_CLIENT_SECRET
              valueFrom: { secretKeyRef: { name: oauth2-proxy, key: client-secret } }
            - name: OAUTH2_PROXY_COOKIE_SECRET
              valueFrom: { secretKeyRef: { name: oauth2-proxy, key: cookie-secret } }
          ports:
            - containerPort: 4180
---
apiVersion: v1
kind: Service
metadata:
  name: oauth2-proxy
  namespace: wshm
spec:
  selector: { app: oauth2-proxy }
  ports:
    - port: 4180
      targetPort: 4180
---
apiVersion: traefik.io/v1alpha1
kind: Middleware
metadata:
  name: oauth2-forward-auth
  namespace: wshm
spec:
  forwardAuth:
    address: http://oauth2-proxy.wshm.svc.cluster.local:4180/oauth2/auth
    trustForwardHeader: true
    authResponseHeaders:
      - X-Auth-Request-User
      - X-Auth-Request-Email
      - Authorization
---
apiVersion: traefik.io/v1alpha1
kind: IngressRoute
metadata:
  name: wshm
  namespace: wshm
spec:
  entryPoints: [websecure]
  routes:
    # OAuth callback path goes straight to oauth2-proxy (no auth middleware).
    - match: Host(`wshm.example.com`) && PathPrefix(`/oauth2`)
      kind: Rule
      services:
        - name: oauth2-proxy
          port: 4180
    # Everything else: ForwardAuth → wshm.
    - match: Host(`wshm.example.com`)
      kind: Rule
      middlewares:
        - name: oauth2-forward-auth
      services:
        - name: wshm
          port: 80
  tls:
    certResolver: your-letsencrypt-resolver
```

Apply it all:

```bash
kubectl apply -f wshm-deployment.yaml
kubectl apply -f oauth2-proxy.yaml
kubectl apply -f ingressroute.yaml
```

Browse `https://wshm.example.com` — you'll be redirected to Google login,
then land on the wshm web UI. The Basic Auth prompt from wshm itself is
*also* triggered (since `WSHM_WEB_PASSWORD` is set). Either remove
`WSHM_WEB_PASSWORD` from the Secret to disable Basic Auth (oauth2-proxy is
already enforcing identity), or use both layers if you want belt-and-suspenders.

## 5. Verify

```bash
kubectl -n wshm get pods,svc,ingressroute
kubectl -n wshm logs deploy/wshm
curl -k https://wshm.example.com/health    # public, returns JSON
```

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| 401 from wshm even after Google login | `WSHM_WEB_PASSWORD` set without `oauth2-proxy --pass-basic-auth=true` |
| 502 from Traefik | `oauth2-proxy` not ready, or Service selector mismatch |
| `Not inside a git repository` in pod logs | `--repo` flag missing on the Deployment `args:` |
| `No GitHub token found` | Secret missing `GITHUB_TOKEN`, or wrong env var name |
| Web UI blank, only `<!doctype html>` 53 bytes | Image built without the bun build step (use `innovtech/wshm:latest` from v0.28.4+) |

## Adding repos at runtime

Once the daemon is running in multi-repo mode (`--config global.toml`), the
Settings page exposes an "Add repository" form (POST `/api/v1/repos`) that
appends to the global config TOML and spawns the scheduler/poller for the
new repo without restarting the pod. In mono-repo mode (`--repo X`), this
endpoint returns 405 — restart the pod after editing the args.
