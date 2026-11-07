# Configuration

wshm is configured via `.wshm/config.toml` in your repository root.

## Initialize

```bash
wshm config init
```

## Full Reference

```toml
[github]
# Token from env: GITHUB_TOKEN or WSHM_TOKEN (never in config)

[ai]
provider = "anthropic"            # anthropic, openai, google, mistral, groq, deepseek, xai, ollama, ...
model = "claude-sonnet-4-20250514"
# base_url = "https://custom-proxy.com/v1"  # optional: custom API endpoint

[ai.rag]                          # "Related history" in triage / PR-analysis prompts
enabled = true                    # closed issues & merged PRs found by full-text search
top_k = 5                         # items kept in the prompt
max_terms = 8                     # search terms extracted from title + body
max_chars = 3000                  # size cap of the block
# Env overrides (beat the file): WSHM_RAG_ENABLED, WSHM_RAG_TOP_K,
# WSHM_RAG_MAX_TERMS, WSHM_RAG_MAX_CHARS
# Per-repo switch: Settings → Repos → "Related history in prompts"
# (features.related_history, default true); both switches must be on.
# Logs: one INFO line per triage/analysis listing the retrieved items,
# the full block at RUST_LOG=debug.

[ai.injection]                    # prompt-injection hardening (defaults shown)
detection = true                  # flag likely injection attempts (rules, no AI call)
threshold = 3                     # score at which an item is flagged (1..=20)
block_mutations = true            # flagged item: no label change, close, auto-fix PR
                                  # or diagram
maintainer_note = true            # add a short warning to the comment wshm posts
require_non_ai_condition = true   # an AI-chosen duplicate close or auto-fix start needs,
                                  # on the issue it acts on: an author who is a
                                  # collaborator or in trusted_authors, or a
                                  # collaborator's approval of its current content.
                                  # The auto-fix itself still applies [fix]
                                  # trusted_authors_only / allowed_users.
approval_label = "wshm:approved"  # approval: added by a collaborator, recorded from
                                  # the GitHub `issues.labeled` webhook event and
                                  # bound to the issue content at that moment (an
                                  # edit voids it); the event re-runs triage. Not
                                  # usable in polling mode, ignored on Azure DevOps
                                  # (any contributor can set tags). Must not be a
                                  # label wshm applies itself. The model can never
                                  # apply it.
trusted_authors = []              # trusted in addition to repository collaborators
# Per-repo override, stored in the DB and shared by every pod:
#   GET/PUT /api/v1/repos/{slug}/injection   (body: partial object, or null;
#   PUT: admin). GET also lists the flagged items; the matched excerpts are
#   only returned to operators and admins, as plain text (escape before
#   rendering).
#
# Slash command: `/wshm fix` refuses a flagged or suspicious issue; a
# maintainer can override with `/wshm fix --force` (CLI: `wshm fix --force`).
#
# Always on, not configurable:
# - issue/PR text, diffs, file paths, files, retrieved history and memory are
#   sent as delimited untrusted data (random boundary per block, invisible,
#   bidi and tag characters removed, lookalike characters folded for
#   detection, hidden comments and collapsed sections surfaced). Only the
#   item's own content decides; suspicious context lines (other titles,
#   history, memory) are left out of the prompt, and flagged items never
#   come back as related history or memory.
# - in test/fixture files of a diff, findings add at most 2 to the score.
# - model output is validated: labels from [[labels]] or, without it, the
#   standard set only; CI/merge trigger labels (ok-to-test, automerge, lgtm,
#   run-ci...) never on PRs; known duplicate target; linked issues referenced
#   in the PR; inline review lines present in the diff. Posted text loses
#   mentions, links and secrets; HTML is escaped (code is kept verbatim).
# - the `claude -p` backend runs without tools or MCP servers, with the
#   system prompt in its own flag, the prompt on stdin, no saved session and
#   an empty working directory.
# These are mitigations, not guarantees: keep automatic actions behind human
# review for untrusted contributors.

[triage]
enabled = true
auto_fix = false                  # attempt auto-fix for simple bugs
auto_fix_confidence = 0.85        # minimum confidence for auto-fix
triage_confidence = 0.5           # minimum confidence to apply triage
labels_bug = "bug"
labels_feature = "feature"
labels_duplicate = "duplicate"
labels_wontfix = "wontfix"
labels_needs_info = "needs-info"
retriage_interval_hours = 0       # re-evaluate every N hours (0 = disabled)

[pr]
enabled = true
auto_label = true
risk_labels = true                # add risk:low / risk:medium / risk:high

# Architecture diagram in the PR analysis comment (Mermaid, inserted before
# the review checklist). One extra AI call per new diff, cached per PR.
# Every key can be overridden per repo in Settings → Diagrams (stored in
# the DB, so all pods share it): DB > this file > defaults.
[pr.review_diagrams]
enabled = true
kinds = ["architecture"]          # only "architecture" is implemented
text = "short"                    # short | normal | full
language = "en"                   # language of the diagram texts
comment = "mermaid"               # mermaid | none (link to the page only)
diff_viewer = true                # embed the diff in the interactive page
active_days = 14                  # skip PRs idle for longer (0 = no limit); a push re-enables
max_mermaid_chars = 5000          # Mermaid size cap (Forgejo/Gitea default limit)
timeout_secs = 120                # budget of the diagram step (runs after the comment is posted)
max_summary_calls = 3             # per-file summary calls for diffs > 40 KB

# Interactive page (React Flow viewer), deleted when the PR is closed.
# Destinations (dir, pages_repo, bucket, region, endpoint, account,
# container, public_url) and allow_private are TOML-only: the per-repo
# override from the dashboard can only choose `target` and the caps.
[pr.review_diagrams.publish]
target = "off"                    # off | local | github-pages | wshm (Enterprise) | s3 | gcs | azure
# s3/gcs/azure: bucket / region / endpoint (S3, GCS) or account / container
# (Azure), objects uploaded as text/html; needs a build with the matching
# storage-s3 / storage-gcs / storage-azure feature. Credentials come from the
# provider's standard chain (env, workload identity), never from this file.
# prefix = "wshm-diagrams"
# public_url = "https://diagrams.example.com"  # link = public_url/<key>; for wshm: the daemon URL
# dir = "/var/www/diagrams"       # local (TOML only, not in the per-repo override)
# pages_repo = "owner/diagrams"   # github-pages: commits to its gh-pages branch (TOML only).
#                                 # If this repo is public, its tree and git history list
#                                 # every page: the random token in the file name does not
#                                 # prevent enumeration there.
# retention_days = 30             # delete pages idle for longer (0 = until the PR closes)
# max_pages = 200                 # per repo, least recently updated pages evicted first
# max_diff_kb = 1024              # bigger diffs: the viewer links to "Files changed"
# allow_private = false           # public targets are refused for private repos (TOML only)

[queue]
enabled = true
merge_threshold = 15              # minimum score to auto-merge
strategy = "rebase"               # merge, rebase, squash

[conflicts]
enabled = true                    # conflict detection (read-only)
# auto_resolve / auto_resolve_confidence: not implemented, ignored

[fix]
# tool = "claude-code"            # or "codex"
# model = "..."                   # override the agent model
# docker_image = "wshm-sandbox:latest"  # used with `wshm fix --docker`
# container_network = "none"     # network for `--docker` (default none: no egress, so the model API is unreachable;
#                                 # set a network with a controlled egress). WSHM_FIX_CONTAINER_NETWORK is a fallback when empty.
# trusted_authors_only = true     # default; WARNING: false lets any issue author steer the agent
# test_command = "cargo test"
# test_retries = 2

[sync]
interval_minutes = 5              # minimum time between auto-syncs
full_sync_interval_hours = 24

[license]
# key = "vault(secret/wshm/license-key)"  # vault placeholder (recommended)
# path = "~/.wshm/license.jwt"            # or explicit JWT path
# Or use env: WSHM_LICENSE_KEY

[vault]
# provider = "hashicorp"          # hashicorp, aws, azure, gcp
# address = "https://vault.company.com"
# mount = "secret"

[web]
# enabled = true
# username = "admin"
# password = "changeme"

[database]
# provider = "sqlite"             # sqlite (default) or postgresql
# uri = "postgres://user:pass@localhost/wshm"

[notify]
# on_run = true                  # `wshm run` sends the summary (the daemon does not)
#
# [[notify.discord]]
# url = "https://discord.com/api/webhooks/ID/TOKEN"
#
# [[notify.slack]]
# url = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"
#
# [[notify.teams]]
# url = "https://outlook.office.com/webhook/YOUR/URL"

[export]
# [export.storage]                # one JSON object per event: <prefix>/YYYY/MM/DD/<Kind>-<ms>.json
# provider = "s3"                 # s3, gcs, azure, local (Enterprise)
# bucket = "wshm-events"          # s3 / gcs
# prefix = "wshm/events"
# region = "eu-west-1"             # s3
# endpoint = "https://s3.gra.io.cloud.ovh.net"  # S3-compatible (MinIO, OVH, Scaleway...)
# account = "mystorageacct"        # azure (or AZURE_STORAGE_ACCOUNT_NAME)
# container = "wshm-events"        # azure
# dir = "/var/lib/wshm/events"     # local
# Credentials are never set here: AWS_* / instance role, GOOGLE_APPLICATION_CREDENTIALS /
# workload identity, AZURE_STORAGE_* / managed identity.
# public_url = "https://cdn.example.com"  # required for links with a custom endpoint
#   (config or env AWS_ENDPOINT*, GOOGLE_BASE_URL, AZURE_STORAGE_ENDPOINT / _USE_EMULATOR)
# S3 credentials: env, web identity, ECS or instance role only (no AWS_PROFILE / ~/.aws/config / SSO).
# A misconfigured sink (missing bucket, invalid prefix...) is logged as an error and skipped;
# wshm still starts.
#
# [export.database]
# provider = "elastic"            # elastic, opensearch, mongodb, mysql (Enterprise)
# uri = "http://localhost:9200"
#
# [[export.webhooks]]
# url = "https://your-server.com/wshm"
# events = ["*"]
# secret = "hmac-secret"

[branding]
# name = "wshm"
# command_prefix = "/wshm"

[labels_blacklist]
# Labels wshm must never apply

[issues_blacklist]
# Issue numbers to never touch

[prs_blacklist]
# PR numbers to never touch
```

## Auto-fix and code execution

The claude agent used by auto-fix may only read and edit files, plus run Bash for the configured `[fix] test_command`. That command still executes repository code (build scripts, test code, package hooks), and the agent can edit that code before running it. This is why `trusted_authors_only = true` (the default) and `wshm fix --docker` (with `container_network = "none"` by default) matter: keep both unless every issue author is trusted.

## Pipelines

Each issue or PR event (webhook or poller) is processed by a **pipeline**: an
ordered list of steps. Without any pipeline config, the built-in `standard`
pipeline runs, and it does exactly what the repo toggles say:

| Item  | Steps of `standard`                                                    |
|-------|------------------------------------------------------------------------|
| issue | `sync` (collect_issues) → `triage` (triage_issues + filters) → `memory` |
| PR    | `sync` (collect_prs) → `review` (review_prs, a failure does not stop the run) → `analyze` (analyze_prs + filters) → `memory` |

So the existing toggles keep working unchanged: `standard` reads
`collect_issues`, `collect_prs`, `triage_issues`, `review_prs`, `analyze_prs`
and the repo filters as step conditions; `review_post_comments` and the apply
mode still decide what the steps post. `auto_pr` and `auto_merge` are not part
of the event pipeline (they were not before either).

**A custom template does not apply the repo toggles by itself.** A step runs
when its own `when` holds: to keep honouring a toggle, list it, e.g.
`when = { features = ["triage_issues"], repo_filters = true }`. Without it, the
step runs even when the toggle is off in Settings → Repos.

The same template drives the other entry points, as a partial run (only the
steps of one kind, recorded with their own trigger):

- the scheduler's periodic triage and retriage run the `triage` steps of each
  pending issue's template (trigger `scheduler`), still behind `[triage]
  enabled`, `triage_issues` and apply mode as before. Such a run is recorded
  only when a step actually runs. `needs` on steps outside the partial run
  (e.g. `sync`) count as met: the scheduler synced already;
- the slash commands `/wshm triage`, `analyze` and `review` run the steps of
  that kind (trigger `command`). Their conditions apply, except the toggles
  (`features`, `repo_filters`), which govern automatic processing and never
  blocked an explicit command. A command on an item whose template has no such
  step, or skips it, answers with the reason. `/wshm review` runs the
  template's whole review chain (`context`, `review`, `security`, `verify`,
  `risk`, `approval`, `status_check`, those the template has), so with a
  collect-mode review it is `verify` that posts; the reply lists what each
  step did. The other commands (`label`, `fix`, `queue`, ...) are not
  pipeline steps;
- a PR review submitted or dismissed (GitHub `pull_request_review`, Gitea /
  Forgejo review events, GitLab MR approve / unapprove) re-runs only the
  `risk`, `approval` and `status_check` steps (trigger
  `pull_request_review.submitted`): no AI call, no review round. Polling mode
  (no webhooks) does not see reviews: the status is updated on the next push.

The pipeline config is only read from `.wshm/config.toml` (default branch
checkout), the global config, or the DB override. Never from the content of an
issue or PR. It holds no secret: steps read credentials from the secret store.

```toml
[[pipelines]]
name = "docs-only"              # a-z 0-9 - _ (1-64 chars); "standard" is reserved
description = "Docs and small PRs"
max_ai_calls = 2                # AI call budget of a run; later steps are skipped

[[pipelines.steps]]
id = "sync"                     # unique in the pipeline
uses = "sync"                   # step kind

[[pipelines.steps]]
id = "analyze"
uses = "analyze"
needs = ["sync"]                # earlier steps that must have finished "done"
# after = ["x"]                 # ordering only: run after x whatever its outcome
on_failure = "continue"         # "abort" (default): stop the run and fail the event
timeout_secs = 300              # the step fails after this long
when = { item = "pr", features = ["analyze_prs"], labels_none = ["skip-ai"] }

[[pipeline_routes]]             # picks the pipeline of a run
template = "docs-only"
priority = 10                   # highest first, then declaration order
when = { item = "pr", paths = ["docs/**", "**/*.md"] }

[pipeline_runs]                 # run history retention
retention_days = 30             # 0 = no age limit
max_per_item = 20               # runs kept per issue/PR, newest first (0 = no limit)
```

No route matches: `standard` runs.

Step kinds: `sync` (refresh the issue/PR from the forge), `triage` (issues),
`review` (PRs, inline review), `analyze` (PRs, analysis and diagrams),
`memory` (store the result in ICM when `[daemon] icm_enabled`), and the review
steps `context`, `security`, `verify`, `risk`, `approval`, `status_check`
(see [Review steps](#review-steps)). A step of the other item kind is left out
of the run.

Conditions (`when`, on steps and routes; all listed must hold):

| Key | Meaning |
|-----|---------|
| `item` | `"issue"` or `"pr"` |
| `features` | repo toggles that must all be on, e.g. `["triage_issues"]` |
| `repo_filters` | apply the repo filters (skip_authors, triage labels/age, target_branches) |
| `labels_any` / `labels_none` | labels on the item (case-insensitive) |
| `authors` / `skip_authors` | item author logins |
| `forges` | `github`, `gitlab`, `gitea`, `forgejo`, `azure-devops` |
| `paths` | PR only: a changed path matches a glob (`*`, `**`, `?`) |
| `risk` | PR only: risk tier (`low`, `medium`, `high`) of the run's `risk` step when it ran, else the risk level of the analysis |
| `repos` | repo slug globs, e.g. `["my-org/*"]` |

A step that does not run is recorded as skipped with the reason
(`review_prs disabled`, `needs 'sync-pr', which was skipped`, ...).

Routes are evaluated before any step, so on a webhook they read the labels and
author from the event payload (a label just added by `pull_request.labeled`
matches); step conditions read the cached item, refreshed by the `sync` step.

Errors never stop the daemon from booting. An invalid TOML pipeline config is
logged and the repo runs `standard` (a valid `[pipeline_runs]` is kept). A
pipeline using an unknown step kind falls back to `standard` (logged). An
unknown name in `when.features` (for instance a toggle renamed by an upgrade) is
a warning and evaluates to false. `GET /api/v1/repos/{slug}/pipelines` shows
them all: `toml_error`, `override_error`, `template_errors`, `warnings`.

Limits:

- `timeout_secs` stops the step where it is: what it already did stays (a label
  set, a comment posted), so partial effects are possible. It is safest on
  `sync` and `memory`; on `triage`, `review` and `analyze` the config gets a
  warning.
- `max_ai_calls` is checked before each step, so the step that crosses the
  budget finishes. The `analyze` step's diagram runs in the background after the
  step: it is not started once the budget is used up, but a started diagram
  (up to `[pr.review_diagrams] max_summary_calls` + 1 calls) is not cut short.
  Its calls are added to the run when it finishes.
- `max_rounds` on a step skips it once it was `done` in that many earlier runs
  of the same template on the same item (e.g. review rounds of a PR), with
  the reason "round cap reached ... ask the author to reconsider the approach".
- `max_ai_calls` on a step caps that step's AI calls (on top of the run's
  budget). Steps that make several calls scale down: `verify` uses fewer
  voters, `security` skips its AI pass and keeps the pattern scan.
- `parallel = true` on a template runs consecutive steps that do not depend
  on each other side by side: a step joins the current group unless it
  `needs` or comes `after` a step of it, or has a `when.risk` condition while
  the group holds a `risk` step. Steps are still recorded in template order.
  The AI calls of the run are counted live across the group; `review`
  (collect mode), `security` and `verify` reserve their calls before making
  them, so together they never go past `max_ai_calls`. The older steps
  (`triage`, `analyze`, post-mode `review`) are checked before they start, so
  one of them can still finish past the budget.
- A failure that the wrapped pipeline already handled by logging and going on
  (a triage or review AI error) is recorded as `failed` but does not abort the
  run, as before.

### Built-in templates

| Template | For | Steps |
|----------|-----|-------|
| `standard` | default | the table above (the repo toggles) |
| `anthropic` | thorough PR review | `sync` → `context` → `review` (collect) ∥ `security` → `verify` ∥ `risk` → `approval` → `status_check`; issues as `standard` |
| `light` | docs and small PRs | `sync` → `analyze` → `risk`, 2 AI calls at most; issues: `sync` → `triage` |
| `triage` | issues | `sync` → `triage` → `memory`; a PR routed here runs no step |

`standard` stays the default: the others only run when a route picks them. A
user template with the name of a built-in one (other than `standard`)
replaces it. `GET /api/v1/repos/{slug}/pipelines` lists them under `builtin`,
with `default_template` (the highest-priority route without conditions, else
`standard`).

`anthropic` follows [how Anthropic reviews code](https://claude.dev/blog/solving-code-review-at-anthropic/):
repository context, bug and security reviews side by side, a verification
vote on every finding (only confirmed ones are posted), findings tagged
Important / Nit / Pre-existing, a risk tier with human approvals per tier,
and a blocking status while important findings stay open. It keeps the
`review_prs` toggle on its review steps and caps the review at 3 rounds per
PR (`max_rounds`). Route PRs to it:

```toml
[[pipeline_routes]]
template = "anthropic"
priority = 10
when = { item = "pr", repos = ["my-org/critical-*"] }

[[pipeline_routes]]
template = "light"
priority = 20
when = { item = "pr", paths = ["docs/**", "**/*.md"] }
```

### Review steps

Every step records its status (done, skipped with the reason, failed), its
duration, AI calls, a summary and its findings count. Text from the PR, the
repository and the model is wrapped as untrusted data, model output is
validated (comments must target a line of the diff; mentions, links and HTML
are removed), and nothing is posted, labelled or set for an item flagged as a
possible prompt injection.

| Kind | What it does | `with` parameters |
|------|--------------|-------------------|
| `context` | Loads `CLAUDE.md` / `REVIEW.md` and the code around the changed lines from the default branch of the local checkout, for the review steps: read from git objects (`origin/HEAD`, else `origin/<[fix] base_branch>`, else the local `<base_branch>`), never from the working tree or the PR; tracked files only, `.wshm/` excluded. Done even when nothing is found. No AI. | `files` (default `["CLAUDE.md", "REVIEW.md"]`), `lines` around each hunk (20), `max_bytes` of code (30000) |
| `review` | The inline review. With `mode = "collect"` the findings are kept for `verify` instead of being posted, and the step's AI calls (one per file) stay within its `max_ai_calls` and the run's budget: files over the limit are not reviewed (said in the summary). | `mode` (`post` default, `collect`) |
| `security` | A separate security review: a pattern scan of the added lines for committed secrets (no AI), then an AI pass on secrets, injections and permissions. Findings go to `verify`. | `ai` (true), `secrets` (true) |
| `verify` | `voters` AI calls vote on every finding of the run; a finding is confirmed with `min_votes` votes (default: a majority of `voters`). When fewer voters than `min_votes` fit in the budget or answer, nothing is confirmed or posted and the reason is recorded (`status_check` then stays pending). Nits beyond `max_nits` are dropped. The confirmed findings are posted as one review (apply mode and `review_post_comments`), each finding once per PR (keyed by path, category, wording and the text of the targeted line; claimed in the DB before posting), and stored for the Review page. | `voters` (3, 1-5), `min_votes`, `max_nits` (3), `post` (true) |
| `risk` | Classifies the change: `high` when a path matches `high_paths`, it changes more than `large_lines` lines, or an important security finding was made; `low` when every path matches `low_paths`; `medium` otherwise. Read by `when.risk` and `approval`. No AI. | `high_paths` (workflows, CI, Dockerfiles, migrations, auth, security, `*.pem`), `low_paths` (`*.md`, `docs/**`, `*.txt`, `*.rst`, `LICENSE*`), `large_lines` (1000), `security_high` (true) |
| `approval` | Human approvals required per tier (default `low = 0`, `medium = 1`, `high = 2`), at least one when a path matches `always_human` or the author is not in `trusted_authors` (when set). Counts only approvals on the head commit by reviewers with write access (GitHub `author_association` OWNER, MEMBER or COLLABORATOR; Gitea / Forgejo `official` reviews); bots and the author never count. wshm itself only posts comment reviews, never approvals. Optionally keeps `label` on the PR while approvals are missing (apply mode). | `low`, `medium`, `high`, `always_human` (workflows, `CODEOWNERS`), `trusted_authors`, `label` |
| `status_check` | Always sets a commit status `context` on the PR head the review saw, so a required check is never left missing: `pending` while the review of that commit is not complete (review, verify or security skipped or failed, round cap, findings not verified), `failure` while important findings stay open (the verified ones when `verify` ran, else the last complete review of the same commit), `pending` while required approvals are missing or could not be counted (`approvals = true`), `success` otherwise. Use `after` (not `needs`) so it runs whatever the review steps did. Make it a required check in the branch protection to block the merge. Apply mode only; not set on forges without commit statuses or on items flagged as possible prompt injection. | `context` (`wshm/review`), `approvals` (true) |

Severity: a finding on a line the PR did not add is `Pre-existing` (not held
against the author and never blocking); on an added line, `error` / `warning`
are `Important` and the rest `Nit`.

Forge support: commit statuses on GitHub, GitLab, Gitea and Forgejo (not Azure
DevOps yet); approval counting on GitHub, Gitea and Forgejo. On GitLab,
approvals are not counted, so `status_check` stays `pending` whenever the tier
requires an approval: set the tiers to 0 or `approvals = false` there. A new
commit re-runs the review on `pull_request.synchronize`; a submitted or
dismissed review re-runs the approval and status steps.

A custom template mixing them:

```toml
[[pipelines]]
name = "strict"
parallel = true
max_ai_calls = 20

[[pipelines.steps]]
id = "sync"
uses = "sync"
when = { item = "pr" }

[[pipelines.steps]]
id = "context"
uses = "context"
needs = ["sync"]
with = { files = ["CLAUDE.md", "REVIEW.md", "docs/ARCHITECTURE.md"], lines = 30 }

[[pipelines.steps]]
id = "review"
uses = "review"
needs = ["context"]
max_rounds = 3
with = { mode = "collect" }

[[pipelines.steps]]
id = "security"
uses = "security"
needs = ["context"]
on_failure = "continue"

[[pipelines.steps]]
id = "verify"
uses = "verify"
needs = ["review"]
max_ai_calls = 5
with = { voters = 5, min_votes = 3, max_nits = 0 }

[[pipelines.steps]]
id = "risk"
uses = "risk"
needs = ["sync"]
with = { high_paths = ["src/billing/**", ".github/**"], large_lines = 600 }

[[pipelines.steps]]
id = "approval"
uses = "approval"
needs = ["risk"]
with = { medium = 1, high = 2, always_human = ["migrations/**"], trusted_authors = ["alice", "bob"], label = "needs-human-review" }

[[pipelines.steps]]
id = "status"
uses = "status_check"
after = ["verify", "approval"]
with = { context = "wshm/review" }
```

### Nightly review

`[pipeline_nightly]` reviews existing code of the default branch outside any
PR, a few files per night, to find old bugs. Off by default.

```toml
[pipeline_nightly]
enabled = true
hour_utc = 3                  # earliest UTC hour; runs once a day, on one pod
paths = ["src/**"]            # tracked files of the default branch matching these globs
exclude = ["src/generated/**"]
max_files = 5                 # one AI call per file; the next night goes on from there
create_issue = false          # apply mode: open one issue listing the findings
```

The scheduler starts it after its sync cycle (so it needs `collect_issues` or
`collect_prs`). Files are read from git objects of the default branch (as for
`context`), never the working tree. Files over 60 KB or 2000 lines, lock files and binaries are
skipped, as are files whose content looks like a prompt injection attempt.
Each night is recorded as a run of item kind `repo`, number 0, template
`nightly`, visible in `GET /api/v1/repos/{slug}/runs` without filter.
`pipeline_nightly` can also be set in the DB override.

DB override (multi-pod, shared by every pod): `PUT /api/v1/repos/{slug}/pipelines`
(admin) with a JSON object holding any of `pipelines`, `pipeline_routes`,
`pipeline_runs`, `pipeline_nightly`; each replaces the TOML value
(`pipeline_runs` and `pipeline_nightly` are merged key by key). `null` drops the override. The PUT rejects an invalid override; one
that became invalid later (for instance after an upgrade) only loses its
invalid sections at run time (`override_error`). `GET` on the same path returns
the effective, TOML and override values, the errors and warnings, and the
built-in templates.

Runs: `GET /api/v1/repos/{slug}/runs?item=N` (optionally `&kind=issue|pr`,
`&limit=50`) returns the runs, newest first: template, trigger, PR head sha,
status, and per step the status (done / skipped / failed), reason, timings,
AI calls, summary and findings count.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` | GitHub personal access token |
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key (if using OpenAI) |
| `WSHM_LICENSE_KEY` | License key (alternative to vault/config) |
| `WSHM_TOKEN` | Alternative to GITHUB_TOKEN |
| `WSHM_RAG_ENABLED` | Override `[ai.rag] enabled` (true/false) |
| `WSHM_RAG_TOP_K` | Override `[ai.rag] top_k` |
| `WSHM_RAG_MAX_TERMS` | Override `[ai.rag] max_terms` |
| `WSHM_RAG_MAX_CHARS` | Override `[ai.rag] max_chars` |
| `VAULT_TOKEN` | HashiCorp Vault token |
| `VAULT_ROLE_ID` | HashiCorp AppRole role ID |
| `VAULT_SECRET_ID` | HashiCorp AppRole secret ID |
| `AWS_ACCESS_KEY_ID` | AWS credentials |
| `AWS_SECRET_ACCESS_KEY` | AWS credentials |
| `AZURE_CLIENT_ID` | Azure service principal |
| `AZURE_CLIENT_SECRET` | Azure service principal |
| `AZURE_TENANT_ID` | Azure tenant |
| `GOOGLE_APPLICATION_CREDENTIALS` | GCP service account JSON path |

## Global Config (Multi-Repo)

For managing multiple repos with the daemon, use `~/.wshm/global.toml`:

```toml
[daemon]
bind = "127.0.0.1:3000"
webhook_secret = "your-secret"
apply = false
poll = true
poll_interval = 30

[[repos]]
slug = "owner/repo-1"
path = "/home/user/repos/repo-1"
enabled = true

[[repos]]
slug = "owner/repo-2"
path = "/home/user/repos/repo-2"
enabled = true
apply = true    # override per-repo
```

## Config Precedence

1. CLI flags (`--apply`, `--offline`, `--repo`)
2. Environment variables
3. `.wshm/config.toml` (per-repo)
4. `~/.wshm/global.toml` (global defaults)
5. Built-in defaults
