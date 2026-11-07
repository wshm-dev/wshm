# Azure DevOps

wshm works with Azure DevOps Services (`dev.azure.com`) and Azure DevOps
Server (on-premises). Issues are **work items** of the project, pull requests
are **Azure Repos** pull requests.

## Repository and URL

```toml
# .wshm/config.toml
git_provider = "azure-devops"
# Azure DevOps Server only (Services defaults to https://dev.azure.com):
# git_url = "https://tfs.example.com/tfs"

[azure_devops]
work_item_types = ["Issue", "Bug"]   # work item types handled as issues
                                     # (Agile: "Issue", "Bug"; Scrum: "Bug";
                                     #  CMMI: "Issue", "Bug"; or "User Story"...)
```

The repository slug is `organization/project/repo`:

```bash
wshm --repo acme/Platform/web sync
```

- Services: `organization` is the name in `https://dev.azure.com/<organization>`.
- Server: `organization` is the collection (`DefaultCollection`), and `git_url`
  is the server URL up to the collection (`https://tfs.example.com/tfs`).
- A repository named like its project can be written `acme/Platform`.
- Names are case-insensitive on Azure, but keep the exact spelling in the slug.

Work item types can also be changed at runtime, per repo and shared by every
pod, with the `azure_work_item_types` app setting (JSON array of strings).

## Personal access token

Create a PAT (User settings → Personal access tokens) for the account wshm
acts as. A dedicated "bot" account is recommended: wshm ignores comments
written by this account.

| Scope | Why |
|-------|-----|
| Work Items: **Read & Write** | sync, tags (labels), comments, close duplicates, create issues |
| Code: **Read & Write** | PRs, diffs, threads (comments, inline review), PR labels, completion |
| Build: **Read** | CI state of PRs (build validation policies) |
| Project and Team: **Read** | project visibility (private projects never go to public diagram pages) |
| Identity: **Read** | permission checks (slash commands, auto-fix, duplicate close) |
| Security: **Manage** | permission checks: reading the repository access control lists |

Without the two last scopes the permission check fails, and every check is
then denied (see [Permissions](#permissions)): wshm still works, but only
users listed in `[fix] allowed_users` / `[ai.injection] trusted_authors` can
run slash commands and trigger AI-chosen actions.

Store it as the `azure_devops_token` secret (Settings → Secrets, per repo or
global) or export `AZURE_DEVOPS_TOKEN` (`AZURE_DEVOPS_EXT_PAT` also works).
It is never read from the config file. Without a token, wshm runs read-only on
public projects.

Azure DevOps Server (`git_url` set to your own host): forge tokens are bound to
their host (introduced with the GitLab work, PR #249). The provider-wide
`azure_devops_token` / `AZURE_DEVOPS_TOKEN` is only sent to `dev.azure.com`
or to a host listed in `WSHM_FORGE_HOSTS`; otherwise store the PAT as
`azure_devops_token@<host>` (for `git_url = "https://tfs.example.com/tfs"`:
`azure_devops_token@tfs.example.com`). A plain `http://` server gets no token
unless `WSHM_ALLOW_HTTP_FORGE=1`: prefer HTTPS, the PAT travels in every
request.

An expired PAT, or one missing a scope, makes Azure answer with a sign-in page
(HTTP 203): wshm reports it as an authentication error.

## What maps to what

| wshm | Azure DevOps |
|------|--------------|
| issue | work item of a type listed in `work_item_types` |
| issue labels | work item tags (`System.Tags`) |
| close issue | first state of the type in the *Completed* category |
| close as duplicate | first state in the *Removed* category, else *Completed* |
| issue assignee | `Assigned To` (a single user) |
| PR labels | PR labels (tags) |
| PR assignee | optional reviewer |
| PR comment | a PR thread; wshm's summary and analysis threads are created *closed*, so they never block the "comments must be resolved" policy |
| inline review comment | an *active* thread anchored on the new file, at the line |
| conflicts | `mergeStatus` (`conflicts` / `succeeded`) |
| CI status | build validation policies, plus PR statuses posted by other tools |
| review decision | reviewer votes (approved / waiting / rejected) |
| merge (`wshm queue --apply`, merge-queue policy of PR #249) | direct completion pinned to the head that was scored (refused when the source branch moved since), squash or merge commit; never auto-complete, never scheduled on CI. Branch policies still apply: Azure refuses a completion they block |
| approvals (merge queue gate) | reviewers who voted *Approved* (10) or *Approved with suggestions* (5) and pass the permission check below; the author, wshm's own account and groups never count |

Not available on Azure DevOps, and skipped with a log line:

- Mermaid diagrams: PR comments do not render Mermaid. The architecture
  section of the analysis links to the published diagram page instead
  (`[pr.review_diagrams.publish]`), or keeps the text only. The
  `github-pages` publish target needs GitHub.
- Reactions (👍 count in the PR score).
- Slash commands in polling mode (no repository-wide comment feed): use
  service hooks.
- The `[ai.injection] approval_label`: any contributor can set tags, so a tag
  is never taken as a maintainer approval.

## Permissions

Slash commands, auto-fix and AI-chosen actions check that the user may push to
the repository. On Azure DevOps a user qualifies when:

1. the identity is found by exact account name or e-mail;
2. the access control lists of the repository and of its parents (all
   repositories of the project, all repositories of the organization) are
   read for the user and every group it belongs to, nested groups included;
3. a *Contribute* deny at any of these levels, for the user or one of its
   groups, denies;
4. otherwise a *Contribute* allow grants, at the repository or at a parent
   it inherits from (members of the project's Contributors group usually
   qualify through the project-level allow).

Anything else is denied, including an evaluation that cannot be completed
(unknown group membership, paged or unreadable access control list, API
error). For other
setups (custom groups, organization-level rights), list the users in
`[fix] allowed_users` or `[ai.injection] trusted_authors`.

## Service hooks (webhook mode)

In the project: Project settings → Service hooks → Create subscription →
**Web Hooks**. Create one subscription per event:

| Event | Filters |
|-------|---------|
| Work item created | the work item types you sync (optional) |
| Work item commented on | (slash commands on issues) |
| Pull request created | repository |
| Pull request updated | repository, **Change: Source branch updated** (updates that leave the head unchanged are acknowledged and dropped anyway) |
| Pull request commented on | repository (slash commands on PRs) |

For each subscription:

- URL: `https://your-server.example.com/webhook`
- Authentication, one of:
  - Basic authentication: any user name, the secret as password;
  - HTTP headers: `X-Wshm-Token: <secret>`.
- Resource details to send: **All**. Messages to send: any.
- Work item events of a project with several repos managed by wshm: add the
  header `X-Wshm-Repo: organization/project/repo` (HTTP headers field, one
  `Name: value` per line) so the event reaches the right repo. With a single
  repo of that project it is routed automatically.

The secret is its own value, distinct from the GitHub webhook secret (it is
visible to the project administrators who manage service hooks): store it as
the `azure_devops_webhook_secret` secret (per repo, or global) or export
`WSHM_AZURE_DEVOPS_WEBHOOK_SECRET`. Credentials are compared in constant time.

Order of the checks:

1. events wshm does not handle are answered 200 "ignored";
2. the delivery is routed to one configured `azure-devops` repo; an
   unroutable delivery is rejected (404) before any secret is looked at;
3. it is authenticated with the secret of that repo (repo scope, then
   global). No secret: rejected (503). `WSHM_ALLOW_UNSIGNED_WEBHOOKS` does not
   apply to service hooks.

Deliveries are deduplicated on the event id, so Azure's retries are processed
once. For comment events, wshm reads the comment back through the API by its
ids and uses the author and text from there, never from the delivery.

## Polling mode

Without service hooks, `wshm daemon --poll` picks up new work items and PRs and
new pushes to open PRs. Slash commands need service hooks.

## Auto-fix

Auto-fix pushes a branch from the local checkout with `git push origin`: the
checkout must exist and its `origin` must be able to push to Azure Repos
(credential helper, or a PAT in the remote URL). The daemon only clones GitHub
repositories automatically.
