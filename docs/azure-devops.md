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
| merge (`wshm queue --apply`) | auto-complete with `[queue] strategy` (`merge`, `squash`, `rebase`, `rebase-merge`): Azure completes the PR once every branch policy passes |

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

1. the identity is found by exact account name or e-mail, and
2. no explicit *Contribute* deny on the repository applies to the user or to
   the project's Contributors / Project Administrators groups, and
3. the user has an explicit *Contribute* allow on the repository, or is a
   member (direct or nested) of the project's **Contributors** or
   **Project Administrators** group.

Anything else is denied, including when the check itself fails. For other
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
| Pull request updated | repository, **Change: Source branch updated** (any other change would re-run the analysis) |
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
`WSHM_AZURE_DEVOPS_WEBHOOK_SECRET`. Without it, service hook deliveries are
rejected (HTTP 503). Credentials are compared in constant time.

Deliveries are deduplicated on the event id, so Azure's retries are processed
once. A service hook can only drive a repo whose `git_provider` is
`azure-devops`.

## Polling mode

Without service hooks, `wshm daemon --poll` picks up new work items and PRs and
new pushes to open PRs. Slash commands need service hooks.

## Auto-fix

Auto-fix pushes a branch from the local checkout with `git push origin`: the
checkout must exist and its `origin` must be able to push to Azure Repos
(credential helper, or a PAT in the remote URL). The daemon only clones GitHub
repositories automatically.
