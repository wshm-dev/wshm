# GitLab

wshm works with GitLab.com and self-managed GitLab (REST API v4). Issues,
merge requests, labels, comments, inline review, merge queue, auto-fix and
webhooks all go through the GitLab API.

## 1. Token

Create a token for the account wshm will act as. A **project access token**
(Settings > Access tokens) is the best choice: it is a bot user limited to one
project.

- Role: **Developer** at least (Maintainer if wshm must merge into protected
  branches).
- Scope: **`api`**. Add **`write_repository`** if you use auto-fix and wshm
  clones or pushes over HTTPS.
- `read_api` is enough for a read-only (dry-run) setup.

Give the token to wshm without writing it in a config file:

- Web UI: Settings > Secrets, key `gitlab_token` (global, or scoped to the repo
  slug), or
- environment: `GITLAB_TOKEN=glpat-...`

`GITHUB_TOKEN` / `WSHM_TOKEN` are never sent to GitLab.

## 2. Mark the repo as GitLab

### Single repo (CLI)

In a checkout whose `origin` is on gitlab.com, nothing is needed: the provider
is detected from the remote. Otherwise, in `.wshm/config.toml`:

```toml
git_provider = "gitlab"
git_url = "https://gitlab.example.com"   # self-managed only; default https://gitlab.com
```

Projects in subgroups keep their full path: `--repo group/subgroup/project`.

### Daemon (multi-repo)

Set the forge on the registry entry in `~/.wshm/global.toml`. It wins over the
checkout's config and is used to clone a missing checkout (auto-fix):

```toml
[[repos]]
slug = "group/subgroup/project"
path = "/srv/wshm/repos/project"
git_provider = "gitlab"
git_url = "https://gitlab.example.com"   # self-managed only
```

Repos added at runtime accept the same fields:

```bash
curl -X POST https://wshm.example.com/api/v1/repos \
  -H 'Content-Type: application/json' -H 'X-Wshm-Csrf: 1' \
  -d '{"slug": "group/subgroup/project", "git_provider": "gitlab", "git_url": "https://gitlab.example.com"}'
```

## 3. Webhook

Without a webhook the daemon polls (issues, MRs and new comments, so slash
commands still work). For real-time events:

1. Pick a random secret and store it in wshm as the secret `gitlab_webhook_token`
   (Settings > Secrets, global or scoped to the project path) or as
   `WSHM_GITLAB_WEBHOOK_TOKEN`. Use a value different from the GitHub
   `webhook_secret`.
2. In GitLab: project Settings > Webhooks > Add new webhook.
   - URL: `https://wshm.example.com/webhook`
   - Secret token: the same value
   - Triggers: **Issues events**, **Merge request events**, **Comments**
   - Keep SSL verification on.

wshm checks the `X-Gitlab-Token` header (constant-time compare) and rejects a
delivery for a project it does not track. What it acts on:

| GitLab event | wshm |
|--------------|------|
| Issue opened | triage |
| Labels added to an issue | records the injection approval label (`wshm:approved`) when added by a Developer or above |
| MR opened / reopened, new commits | PR analysis (and review if enabled) |
| MR closed or merged | refresh, diagram page cleanup |
| New comment on an issue or MR | slash commands (`/wshm ...`) |

Every other authenticated delivery (pipeline, MR approval, edits, confidential
issues and comments, ...) is answered `200 ignored`, so GitLab never disables
the hook. CI status comes from the sync, not from Pipeline events.

## Behaviour on GitLab

- MRs are shown as `!N` in the web UI, with links to `/-/merge_requests/N`.
- wshm's comments are notes carrying a hidden marker; the analysis note is
  updated in place, never a review note or an inline discussion.
- Inline review comments are diff discussions anchored on the MR's
  `base_sha` / `start_sha` / `head_sha` (renamed files and unchanged lines
  included). Suggestions use `suggestion:-0+0`. A comment GitLab cannot
  anchor is kept in the review summary.
- Mermaid diagrams render in notes.
- "Collaborator" (slash commands, injection guard, approval label) means a
  project member with **Developer (30) or above**, inherited group membership
  included.
- Merge queue: `wshm queue --apply` merges the top MR above
  `[queue].merge_threshold`, pinned to the head SHA that was scored. It never
  merges an MR with conflicts or failed CI; with a pipeline still running it
  sets "merge when pipeline succeeds". `[queue].strategy = "squash"` squashes;
  other strategies use the project's merge method.
- Auto-fix opens a draft MR (`Draft:` title prefix).
- Rate limits: a `429` answer is retried after `Retry-After` (or
  `RateLimit-Reset`), at most 60 s.
