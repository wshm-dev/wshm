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

A token is only sent to the host it is meant for:

- `gitlab_token` / `GITLAB_TOKEN` go to gitlab.com, and to a self-managed
  host only when that host is listed in `WSHM_FORGE_HOSTS`
  (comma-separated, e.g. `WSHM_FORGE_HOSTS=gitlab.example.com`).
- A token stored as `gitlab_token@<host>` (e.g.
  `gitlab_token@gitlab.example.com`) goes to that host only. This is the
  simplest setup for a self-managed instance.
- A plain `http://` URL never gets a token unless `WSHM_ALLOW_HTTP_FORGE=1`.

`GITHUB_TOKEN` / `WSHM_TOKEN` are never sent to GitLab.

## 2. Mark the repo as GitLab

### Single repo (CLI)

In a checkout whose `origin` is on gitlab.com, nothing is needed: the provider
is detected from the remote. Otherwise, in `.wshm/config.toml`:

```toml
git_provider = "gitlab"
git_url = "https://gitlab.example.com"   # self-managed only; default https://gitlab.com
```

With `git_provider` set and no `git_url`, the URL is taken from the
`origin` host. A `git_url` whose host differs from `origin` is refused.

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

Without an explicit `path`, a repo added at runtime gets the directory
`repos/<slug with / replaced by __>` (e.g. `repos/group__subgroup__project`).
Two entries can never share a directory. Repos added at runtime accept the
same fields (`git_url` must be `https://`):

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
- Merge queue: merging is opt-in. With `[queue] auto_merge = true`,
  `wshm queue --apply` (and `wshm run --apply`) merges the top MR above
  `[queue].merge_threshold` only when all of these hold:
  - the head is still the one that was scored (the merge is pinned to it);
  - there are no conflicts;
  - the head pipeline succeeded (a running pipeline means wait for the next
    run: wshm never schedules "merge when pipeline succeeds");
  - the MR is not flagged by the prompt-injection guard;
  - it has at least `[queue] required_approvals` (default 1) approvals from
    members with Developer access or above. The author, bots and wshm's own
    account do not count.

  The score never replaces an approval. Without `auto_merge`, the queue
  prints what it would merge. `[queue].strategy = "squash"` squashes; other
  strategies use the project's merge method. If the merge call fails, wshm
  re-reads the MR and reports whether it was merged, without re-sending the
  merge.
- Auto-fix opens a draft MR (`Draft:` title prefix).
- Rate limits: a `429` answer is retried (creates included: GitLab did not
  process the request) after `Retry-After` (or `RateLimit-Reset`), at most
  60 s, within the retry policy's attempts. No wait after the last attempt.
