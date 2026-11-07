//! Azure DevOps provider — REST API 7.1 (Azure DevOps Services and Server).
//!
//! Repository slug: `org/project/repo` (`repo_owner` = organization or
//! collection, `repo_name` = `project/repo`; `project` alone means the repo
//! has the project's name). Issues are work items of the project; pull
//! requests are Azure Repos PRs. Labels on work items are tags.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::StreamExt;
use reqwest::Method;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::config::Config;
use crate::db::backend::DatabaseBackend;
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;
use crate::github::issues::ensure_comment_marker;

use super::http::{self, Auth, Rest};
use super::{Capability, CommentRef, GitProvider, MergeOutcome, MergedPr, PolledComment, Thread};

const API: &str = "7.1";
/// Work item comments are still a preview API in 7.1.
const COMMENTS_API: &str = "7.1-preview.4";
/// PR labels are a preview API in 7.1.
const LABELS_API: &str = "7.1-preview.1";
/// "Git Repositories" security namespace and its GenericContribute bit.
const GIT_NAMESPACE: &str = "2e9eb7ed-3c0a-47d4-87c1-0ffdd275fd87";
const GENERIC_CONTRIBUTE: u64 = 4;
/// Work item states treated as closed across the Basic/Agile/Scrum/CMMI
/// processes.
const CLOSED_STATES: [&str; 5] = ["Closed", "Done", "Removed", "Resolved", "Completed"];
const PAGE: u32 = 100;
/// Ids per WIQL query (the service caps a query at 20 000 results); larger
/// sets are paged by id.
const WIQL_PAGE: usize = 20_000;
/// Policy evaluations are a preview API in 7.1.
const POLICY_API: &str = "7.1-preview.1";
/// Policy type of "Build validation" (Azure Pipelines runs on the PR).
const BUILD_POLICY_TYPE: &str = "0609b952-1397-4640-95ec-e00a01b2c241";
/// PR thread status: 1 active (inline findings), 4 closed (wshm's summary
/// and analysis comments, so they never block a "comments must be
/// resolved" branch policy).
const THREAD_ACTIVE: u8 = 1;
const THREAD_CLOSED: u8 = 4;
const PER_PR_CONCURRENCY: usize = 6;
const PER_PR_CAP: usize = 200;
/// Files fetched to rebuild a PR diff (Azure has no unified-diff endpoint).
const MAX_DIFF_FILES: usize = 300;

pub struct AzureDevOpsProvider {
    rest: Rest,
    base: String,
    org: String,
    project: String,
    repo: String,
    marker: String,
    slug: String,
    /// `[azure_devops] work_item_types` (TOML default).
    work_item_types: Vec<String>,
    /// Repo DB for the runtime override of `work_item_types`.
    db: Option<Arc<dyn DatabaseBackend>>,
    /// Ids per WIQL page (a field so tests can page with a few items).
    wiql_page: usize,
    /// (repository id, project id), resolved once.
    ids: tokio::sync::OnceCell<(String, String)>,
    /// The PAT owner (identity id, unique name), resolved once.
    me: tokio::sync::OnceCell<Option<(String, String)>>,
}

impl AzureDevOpsProvider {
    pub fn new(
        config: &Config,
        base_url: Option<&str>,
        db: Option<Arc<dyn DatabaseBackend>>,
    ) -> Result<Self> {
        let base = base_url.unwrap_or("https://dev.azure.com");
        let auth = match super::forge_token(config, "azure-devops") {
            Some(t) => Auth::BasicPassword(t),
            None => {
                warn!(
                    "Azure DevOps provider built without a PAT — anonymous (public projects \
                     only). Add an azure_devops_token in Settings → Secrets."
                );
                Auth::None
            }
        };
        let mut p = Self::with_rest(
            Rest::new("Azure DevOps", auth)?,
            base,
            &config.repo_owner,
            &config.repo_name,
            config.branding.comment_marker(),
        );
        p.work_item_types = config.azure_devops.work_item_types.clone();
        p.db = db;
        info!(
            "Azure DevOps provider initialized: {}/{}/{} repo {}",
            p.base, p.org, p.project, p.repo
        );
        Ok(p)
    }

    pub(crate) fn with_rest(
        rest: Rest,
        base: &str,
        org: &str,
        project_and_repo: &str,
        marker: String,
    ) -> Self {
        let (project, repo) = match project_and_repo.split_once('/') {
            Some((p, r)) => (p.to_string(), r.to_string()),
            None => (project_and_repo.to_string(), project_and_repo.to_string()),
        };
        Self {
            rest,
            base: base.trim_end_matches('/').to_string(),
            org: org.to_string(),
            project,
            repo,
            marker,
            slug: format!("{org}/{project_and_repo}"),
            work_item_types: crate::config::AzureDevOpsConfig::default().work_item_types,
            db: None,
            wiql_page: WIQL_PAGE,
            ids: tokio::sync::OnceCell::new(),
            me: tokio::sync::OnceCell::new(),
        }
    }

    /// Work item types handled as issues: the DB app setting (shared by all
    /// pods) wins over the TOML list; an empty or invalid value is ignored.
    fn item_types(&self) -> Vec<String> {
        let from_db = self.db.as_ref().and_then(|db| {
            let raw = db
                .get_app_setting(crate::db::settings::AZURE_WORK_ITEM_TYPES_KEY)
                .ok()
                .flatten()?;
            serde_json::from_str::<Vec<String>>(&raw)
                .ok()
                .filter(|v| !v.is_empty())
        });
        from_db
            .or_else(|| Some(self.work_item_types.clone()).filter(|v| !v.is_empty()))
            .unwrap_or_else(|| crate::config::AzureDevOpsConfig::default().work_item_types)
    }

    /// WIQL clause restricting to [`Self::item_types`] (quotes escaped).
    fn type_clause(&self) -> String {
        let list = self
            .item_types()
            .iter()
            .map(|t| format!("'{}'", t.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" AND [System.WorkItemType] IN ({list})")
    }

    fn with_version(url: String, version: &str) -> String {
        let sep = if url.contains('?') { '&' } else { '?' };
        format!("{url}{sep}api-version={version}")
    }

    fn org_url(&self, path: &str, version: &str) -> String {
        Self::with_version(
            format!("{}/{}/_apis/{path}", self.base, enc(&self.org)),
            version,
        )
    }

    fn project_url(&self, path: &str, version: &str) -> String {
        Self::with_version(
            format!(
                "{}/{}/{}/_apis/{path}",
                self.base,
                enc(&self.org),
                enc(&self.project)
            ),
            version,
        )
    }

    fn repo_url(&self, path: &str) -> String {
        self.repo_url_v(path, API)
    }

    fn repo_url_v(&self, path: &str, version: &str) -> String {
        let path = if path.is_empty() {
            String::new()
        } else {
            format!("/{path}")
        };
        self.project_url(
            &format!("git/repositories/{}{path}", enc(&self.repo)),
            version,
        )
    }

    /// Identity service host: `vssps.dev.azure.com` on Services, the same
    /// host on Azure DevOps Server.
    fn identities_url(&self, filter: &str, membership: &str) -> String {
        let host = if self.base == "https://dev.azure.com" {
            format!("https://vssps.dev.azure.com/{}", enc(&self.org))
        } else {
            format!("{}/{}", self.base, enc(&self.org))
        };
        format!(
            "{host}/_apis/identities?searchFilter=General&filterValue={}&queryMembership={membership}&api-version={API}",
            enc(filter)
        )
    }

    /// The identity whose account name or mail is exactly `name` (the
    /// identity search is fuzzy: a near match must never stand in for the
    /// user). `membership` = `None` or `ExpandedUp` (fills `memberOf`).
    async fn find_identity(&self, name: &str, membership: &str) -> Result<Option<Value>> {
        let resp = self
            .rest
            .get_json(&self.identities_url(name, membership))
            .await?;
        Ok(resp["value"].as_array().and_then(|a| {
            a.iter()
                .find(|i| {
                    ["Account", "Mail"].iter().any(|k| {
                        i["properties"][k]["$value"]
                            .as_str()
                            .is_some_and(|v| v.eq_ignore_ascii_case(name))
                    })
                })
                .cloned()
        }))
    }

    /// (repository id, project id), fetched once.
    async fn repo_ids(&self) -> Result<(String, String)> {
        self.ids
            .get_or_try_init(|| async {
                let repo = self.rest.get_json(&self.repo_url("")).await?;
                match (repo["id"].as_str(), repo["project"]["id"].as_str()) {
                    (Some(r), Some(p)) => Ok((r.to_string(), p.to_string())),
                    _ => anyhow::bail!("Azure DevOps: repository id/project id missing"),
                }
            })
            .await
            .cloned()
    }

    /// The PAT owner (identity id, unique name) from `connectionData`,
    /// fetched once. `None` when anonymous or unresolvable.
    async fn me(&self) -> Option<(String, String)> {
        if !self.rest.is_authenticated() {
            return None;
        }
        self.me
            .get_or_init(|| async {
                let url = format!("{}/{}/_apis/connectionData", self.base, enc(&self.org));
                match self.rest.get_json(&url).await {
                    Ok(v) => {
                        let u = &v["authenticatedUser"];
                        let id = u["id"].as_str()?.to_string();
                        let name = u["properties"]["Account"]["$value"]
                            .as_str()
                            .or_else(|| u["providerDisplayName"].as_str())?
                            .to_string();
                        Some((id, name))
                    }
                    Err(e) => {
                        warn!("Azure DevOps: cannot resolve the PAT owner: {e:#}");
                        None
                    }
                }
            })
            .await
            .clone()
    }

    // ── work items ──────────────────────────────────────────────────────

    /// Ids matching `condition`, newest id first. WIQL has no continuation
    /// token and caps a query at 20 000 results, so pages are chained by
    /// id (`[System.Id] < last id of the previous page`).
    async fn wiql_ids(&self, condition: &str) -> Result<Vec<u64>> {
        let types = self.type_clause();
        let url = self.project_url(
            &format!("wit/wiql?timePrecision=true&$top={}", self.wiql_page),
            API,
        );
        let mut out: Vec<u64> = Vec::new();
        for _ in 0..http::MAX_PAGES {
            let keyset = out
                .last()
                .map(|id| format!(" AND [System.Id] < {id}"))
                .unwrap_or_default();
            let query = format!(
                "SELECT [System.Id] FROM WorkItems WHERE [System.TeamProject] = @project{types}{condition}{keyset} \
                 ORDER BY [System.Id] DESC"
            );
            let resp = self
                .rest
                .send_json(Method::POST, &url, &json!({ "query": query }))
                .await?;
            let page: Vec<u64> = resp["workItems"]
                .as_array()
                .map(|a| a.iter().filter_map(|w| w["id"].as_u64()).collect())
                .unwrap_or_default();
            let full = page.len() >= self.wiql_page;
            out.extend(page);
            if !full {
                break;
            }
        }
        Ok(out)
    }

    async fn work_items(&self, ids: &[u64]) -> Result<Vec<Value>> {
        let mut out = Vec::with_capacity(ids.len());
        // Work Items - List accepts at most 200 ids per call.
        for chunk in ids.chunks(200) {
            let list = chunk
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let url = self.project_url(&format!("wit/workitems?ids={list}&errorPolicy=omit"), API);
            let resp = self.rest.get_json(&url).await?;
            out.extend(
                resp["value"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|v| !v.is_null()),
            );
        }
        Ok(out)
    }

    async fn issues_where(&self, condition: &str) -> Result<Vec<Issue>> {
        let ids = self.wiql_ids(condition).await?;
        Ok(self
            .work_items(&ids)
            .await?
            .iter()
            .map(parse_work_item)
            .collect())
    }

    async fn work_item(&self, id: u64) -> Result<Option<Value>> {
        let v = self
            .rest
            .get_json_opt(&self.project_url(&format!("wit/workitems/{id}"), API))
            .await?;
        // Ids are organization-wide: only accept items of this project.
        Ok(v.filter(|w| {
            w["fields"]["System.TeamProject"]
                .as_str()
                .is_none_or(|p| p.eq_ignore_ascii_case(&self.project))
        }))
    }

    async fn patch_work_item(&self, id: u64, ops: Value) -> Result<Value> {
        self.rest.require_auth("Azure DevOps work item update")?;
        self.rest
            .send_json_as(
                Method::PATCH,
                &self.project_url(&format!("wit/workitems/{id}"), API),
                &ops,
                Some("application/json-patch+json"),
            )
            .await
    }

    /// Read-modify-write of `System.Tags`, guarded by the revision so a
    /// concurrent edit fails instead of being overwritten.
    async fn edit_tags(&self, id: u64, edit: impl FnOnce(&mut Vec<String>)) -> Result<()> {
        let item = self
            .work_item(id)
            .await?
            .with_context(|| format!("Azure DevOps work item {id} not found in project"))?;
        let mut tags = split_tags(item["fields"]["System.Tags"].as_str().unwrap_or(""));
        let before = tags.clone();
        edit(&mut tags);
        if tags == before {
            return Ok(());
        }
        let rev = item["rev"].as_u64().unwrap_or(0);
        self.patch_work_item(
            id,
            json!([
                {"op": "test", "path": "/rev", "value": rev},
                {"op": "add", "path": "/fields/System.Tags", "value": tags.join("; ")}
            ]),
        )
        .await?;
        Ok(())
    }

    // ── pull requests ───────────────────────────────────────────────────

    /// List PRs for `status` (active/completed/abandoned/all) with $top/$skip.
    async fn list_prs(&self, status: &str, mut on_item: impl FnMut(&Value) -> bool) -> Result<()> {
        let mut skip = 0u32;
        for _ in 0..http::MAX_PAGES {
            let url = self.repo_url(&format!(
                "pullrequests?searchCriteria.status={status}&$top={PAGE}&$skip={skip}"
            ));
            let resp = self.rest.get_json(&url).await?;
            let items = resp["value"].as_array().cloned().unwrap_or_default();
            for item in &items {
                if !on_item(item) {
                    return Ok(());
                }
            }
            if (items.len() as u32) < PAGE {
                return Ok(());
            }
            skip += PAGE;
        }
        Ok(())
    }

    async fn active_prs(&self) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        self.list_prs("active", |v| {
            out.push(v.clone());
            true
        })
        .await?;
        Ok(out)
    }

    async fn threads(&self, pr: u64) -> Result<Vec<Value>> {
        let resp = self
            .rest
            .get_json(&self.repo_url(&format!("pullRequests/{pr}/threads")))
            .await?;
        Ok(resp["value"].as_array().cloned().unwrap_or_default())
    }

    async fn post_thread(
        &self,
        pr: u64,
        content: &str,
        context: Option<Value>,
        status: u8,
    ) -> Result<()> {
        let mut body = json!({
            "comments": [{"parentCommentId": 0, "content": content, "commentType": 1}],
            "status": status,
        });
        if let Some(ctx) = context {
            body["threadContext"] = ctx;
        }
        self.rest
            .send_json(
                Method::POST,
                &self.repo_url(&format!("pullRequests/{pr}/threads")),
                &body,
            )
            .await?;
        Ok(())
    }

    async fn file_at(&self, path: &str, commit: &str) -> Result<String> {
        let url = self.repo_url(&format!(
            "items?path={}&versionDescriptor.version={commit}&versionDescriptor.versionType=commit&$format=octetStream",
            enc(path)
        ));
        self.rest
            .get_text(&url, Some("application/octet-stream"))
            .await
    }

    async fn identity_id(&self, name: &str) -> Result<String> {
        self.find_identity(name, "None")
            .await?
            .and_then(|i| i["id"].as_str().map(String::from))
            .with_context(|| format!("Azure DevOps identity '{name}' not found"))
    }

    /// Move a work item to the first state of its type in one of
    /// `categories` (tried in order): state names depend on the process
    /// ("Closed", "Done", "Removed", ...), state categories do not.
    async fn close_in_category(&self, number: u64, categories: &[&str]) -> Result<()> {
        let item = self
            .work_item(number)
            .await?
            .with_context(|| format!("Azure DevOps work item {number} not found in project"))?;
        let kind = item["fields"]["System.WorkItemType"]
            .as_str()
            .context("work item type missing")?;
        let states = self
            .rest
            .get_json(&self.project_url(&format!("wit/workitemtypes/{}/states", enc(kind)), API))
            .await?;
        let list = states["value"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let state = categories
            .iter()
            .find_map(|c| list.iter().find(|s| s["category"] == *c))
            .and_then(|s| s["name"].as_str())
            .with_context(|| format!("no {categories:?} state for work item type {kind}"))?;
        if item["fields"]["System.State"].as_str() == Some(state) {
            return Ok(());
        }
        self.patch_work_item(
            number,
            json!([{"op": "add", "path": "/fields/System.State", "value": state}]),
        )
        .await?;
        Ok(())
    }

    async fn upsert_comment(&self, thread: Thread, body: &str) -> Result<()> {
        self.rest.require_auth("Azure DevOps comment")?;
        let body = ensure_comment_marker(body, &self.marker);
        if let Some(existing) = self.find_comment_with_marker(thread, &self.marker).await? {
            info!(
                "Updating existing wshm comment {existing} on #{}",
                thread.number()
            );
            return self.update_comment(&existing, &body).await;
        }
        match thread {
            Thread::Pull(n) => self.post_thread(n, &body, None, THREAD_CLOSED).await,
            Thread::Issue(n) => {
                self.rest
                    .send_json(
                        Method::POST,
                        &self.project_url(
                            &format!("wit/workItems/{n}/comments?format=markdown"),
                            COMMENTS_API,
                        ),
                        &json!({ "text": body }),
                    )
                    .await?;
                Ok(())
            }
        }
    }
}

fn enc(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Identity descriptors come as `"type;identifier"` strings, or as
/// `{identityType, identifier}` objects in some API versions.
fn descriptor_str(v: &Value) -> Option<String> {
    v.as_str().map(String::from).or_else(|| {
        Some(format!(
            "{};{}",
            v["identityType"].as_str()?,
            v["identifier"].as_str()?
        ))
    })
}

/// WIQL date literal (UTC, `Z` suffix) of an RFC 3339 timestamp.
fn wiql_date(ts: &str) -> Option<String> {
    http::parse_ts(ts).map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// Plain text of a work item comment (HTML), for command parsing.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();
    for br in ["<br>", "<br/>", "<br />", "</div>", "</p>"] {
        s = s.replace(br, "\n");
    }
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

/// CI bucket of a build-validation policy evaluation.
fn policy_bucket(status: &str) -> Option<&'static str> {
    match status {
        "approved" => Some("success"),
        "rejected" | "broken" => Some("failure"),
        "queued" | "running" => Some("pending"),
        _ => None,
    }
}

fn split_tags(tags: &str) -> Vec<String> {
    tags.split(';')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

fn is_type_missing(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}");
    msg.contains("(404") || msg.contains("(400")
}

/// Work item descriptions are HTML: escape the Markdown text and keep line
/// breaks, leaving wshm's marker as a real (hidden) HTML comment.
fn markdown_to_html(body: &str, marker: &str) -> String {
    let text = body.replace(marker, "");
    let escaped = text
        .trim_end()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', "<br/>");
    format!("<div>{escaped}</div>{marker}")
}

fn strip_ref(r: &str) -> String {
    r.strip_prefix("refs/heads/").unwrap_or(r).to_string()
}

fn parse_work_item(w: &Value) -> Issue {
    let f = &w["fields"];
    let state = f["System.State"].as_str().unwrap_or("");
    let created_by = &f["System.CreatedBy"];
    Issue {
        number: w["id"].as_u64().unwrap_or(0),
        title: f["System.Title"].as_str().unwrap_or("").to_string(),
        body: f["System.Description"].as_str().map(String::from),
        state: if CLOSED_STATES.contains(&state) {
            "closed"
        } else {
            "open"
        }
        .to_string(),
        labels: split_tags(f["System.Tags"].as_str().unwrap_or("")),
        author: created_by["uniqueName"]
            .as_str()
            .or_else(|| created_by.as_str())
            .map(String::from),
        created_at: f["System.CreatedDate"].as_str().unwrap_or("").to_string(),
        updated_at: f["System.ChangedDate"].as_str().unwrap_or("").to_string(),
        reactions_plus1: 0,
        reactions_total: 0,
    }
}

fn parse_pr(pr: &Value) -> PullRequest {
    let s = |v: &Value| v.as_str().map(String::from);
    let created = pr["creationDate"].as_str().unwrap_or("").to_string();
    PullRequest {
        number: pr["pullRequestId"].as_u64().unwrap_or(0),
        title: pr["title"].as_str().unwrap_or("").to_string(),
        body: s(&pr["description"]),
        state: if pr["status"] == "active" {
            "open"
        } else {
            "closed"
        }
        .to_string(),
        labels: http::label_names(&pr["labels"]),
        author: s(&pr["createdBy"]["uniqueName"]),
        head_sha: s(&pr["lastMergeSourceCommit"]["commitId"]),
        base_sha: s(&pr["lastMergeTargetCommit"]["commitId"]),
        head_ref: pr["sourceRefName"].as_str().map(strip_ref),
        base_ref: pr["targetRefName"].as_str().map(strip_ref),
        mergeable: match pr["mergeStatus"].as_str() {
            Some("succeeded") => Some(true),
            Some("conflicts") => Some(false),
            _ => None,
        },
        ci_status: None,
        // No "last updated" field on Azure PRs: closing is the last change
        // we can observe; active PRs are always re-listed in full.
        updated_at: pr["closedDate"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| created.clone()),
        created_at: created,
        review_decision: None,
        review_decision_at: None,
        reactions_plus1: 0,
    }
}

/// Review decision from reviewer votes: 10 approved, 5 approved with
/// suggestions, 0 no vote, -5 waiting for author, -10 rejected.
fn decision_from_votes(pr: &Value) -> Option<&'static str> {
    let votes: Vec<i64> = pr["reviewers"]
        .as_array()?
        .iter()
        .filter_map(|r| r["vote"].as_i64())
        .collect();
    if votes.is_empty() {
        None
    } else if votes.iter().any(|v| *v < 0) {
        Some("changes_requested")
    } else if votes.iter().any(|v| *v > 0) {
        Some("approved")
    } else {
        Some("review_required")
    }
}

#[async_trait]
impl GitProvider for AzureDevOpsProvider {
    fn provider_name(&self) -> &str {
        "azure-devops"
    }

    fn repo_slug(&self) -> String {
        self.slug.clone()
    }

    fn comment_marker(&self) -> &str {
        &self.marker
    }

    fn is_authenticated(&self) -> bool {
        self.rest.is_authenticated()
    }

    fn supports(&self, cap: Capability) -> bool {
        match cap {
            // Mermaid is documented for the Wiki only (`::: mermaid`), not
            // for PR comments / work item discussions.
            Capability::Mermaid => false,
            Capability::ReviewDecisions | Capability::CiStatus => true,
            // No reactions on PRs; no repo-wide comment feed to poll.
            Capability::Reactions | Capability::CommentPolling => false,
            // Direct completion pinned to the scored head; approvals
            // (fetch_pr_approvers) from reviewer votes of users with
            // Contribute.
            Capability::Merge => true,
        }
    }

    async fn self_username(&self) -> Option<String> {
        self.me().await.map(|(_, name)| name)
    }

    async fn fetch_repo_private(&self) -> Result<bool> {
        let project = self
            .rest
            .get_json(&self.org_url(&format!("projects/{}", enc(&self.project)), API))
            .await?;
        match project["visibility"].as_str() {
            Some(v) => Ok(!v.eq_ignore_ascii_case("public")),
            None => anyhow::bail!("Azure DevOps: project visibility missing"),
        }
    }

    async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        let mut cond = format!(
            " AND [System.State] NOT IN ({})",
            CLOSED_STATES.map(|s| format!("'{s}'")).join(", ")
        );
        if let Some(s) = since.and_then(wiql_date) {
            cond.push_str(&format!(" AND [System.ChangedDate] >= '{s}'"));
        }
        self.issues_where(&cond).await
    }

    async fn fetch_all_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        let cond = since
            .and_then(wiql_date)
            .map(|s| format!(" AND [System.ChangedDate] >= '{s}'"))
            .unwrap_or_default();
        self.issues_where(&cond).await
    }

    async fn fetch_issue(&self, number: u64) -> Result<Option<Issue>> {
        // Only configured types are issues (no Tasks, Epics, Test Cases...).
        let types = self.item_types();
        Ok(self
            .work_item(number)
            .await?
            .filter(|w| {
                w["fields"]["System.WorkItemType"]
                    .as_str()
                    .is_some_and(|t| types.iter().any(|k| k.eq_ignore_ascii_case(t)))
            })
            .map(|w| parse_work_item(&w)))
    }

    async fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        self.rest.require_auth("Azure DevOps create work item")?;
        let mut ops = vec![
            json!({"op": "add", "path": "/fields/System.Title", "value": title}),
            json!({"op": "add", "path": "/fields/System.Description",
                   "value": markdown_to_html(&ensure_comment_marker(body, &self.marker), &self.marker)}),
        ];
        if !labels.is_empty() {
            ops.push(
                json!({"op": "add", "path": "/fields/System.Tags", "value": labels.join("; ")}),
            );
        }
        // First configured type the process has (default Issue, then Bug:
        // Scrum has no "Issue").
        let mut last_err = None;
        for kind in self.item_types() {
            let url = self.project_url(&format!("wit/workitems/${kind}"), API);
            match self
                .rest
                .send_json_as(
                    Method::POST,
                    &url,
                    &json!(ops),
                    Some("application/json-patch+json"),
                )
                .await
            {
                Ok(v) => return v["id"].as_u64().context("work item id missing"),
                // Only an unknown type (400/404) moves on to the next one: any
                // other failure may have created the item already.
                Err(e) if is_type_missing(&e) => last_err = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("work item creation failed")))
    }

    async fn close_issue(&self, number: u64) -> Result<()> {
        self.close_in_category(number, &["Completed"]).await
    }

    async fn close_issue_not_planned(&self, number: u64) -> Result<()> {
        // Duplicates go to the "Removed" category when the process has one
        // (Agile/Scrum/CMMI "Removed"), else to "Completed" (Basic).
        self.close_in_category(number, &["Removed", "Completed"])
            .await
    }

    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        self.edit_tags(number, |tags| {
            for l in labels {
                if !tags.iter().any(|t| t.eq_ignore_ascii_case(l)) {
                    tags.push(l.clone());
                }
            }
        })
        .await
    }

    async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        self.edit_tags(number, |tags| {
            tags.retain(|t| !t.eq_ignore_ascii_case(label))
        })
        .await
    }

    async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        // A work item has a single "Assigned To".
        let Some(first) = assignees.first() else {
            return Ok(());
        };
        if assignees.len() > 1 {
            anyhow::bail!(
                "Azure DevOps work items take a single assignee; got {assignees:?} for #{number}"
            );
        }
        self.patch_work_item(
            number,
            json!([{"op": "add", "path": "/fields/System.AssignedTo", "value": first}]),
        )
        .await?;
        Ok(())
    }

    async fn comment_issue(&self, number: u64, body: &str) -> Result<()> {
        self.upsert_comment(Thread::Issue(number), body).await
    }

    async fn fetch_pulls(&self) -> Result<Vec<PullRequest>> {
        self.fetch_pulls_incremental("all", None).await
    }

    async fn fetch_pulls_incremental(
        &self,
        state: &str,
        since: Option<&str>,
    ) -> Result<Vec<PullRequest>> {
        let mut out: Vec<PullRequest> = Vec::new();
        if state != "closed" {
            out.extend(self.active_prs().await?.iter().map(parse_pr));
        }
        if state != "open" {
            // Closed PRs come newest-closed first; stop at `since`.
            for status in ["completed", "abandoned"] {
                self.list_prs(status, |pr| {
                    let p = parse_pr(pr);
                    if let Some(s) = since {
                        if http::ts_before(&p.updated_at, s) {
                            return false;
                        }
                    }
                    out.push(p);
                    true
                })
                .await?;
            }
        }
        Ok(out)
    }

    async fn fetch_pull(&self, number: u64) -> Result<Option<PullRequest>> {
        Ok(self
            .rest
            .get_json_opt(&self.repo_url(&format!("pullrequests/{number}")))
            .await?
            .map(|v| parse_pr(&v)))
    }

    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>> {
        Ok(self.fetch_pull(number).await?.and_then(|p| p.mergeable))
    }

    async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        // No unified-diff endpoint: take the latest iteration's change list
        // and diff merge-base vs source blobs locally with libgit2.
        let iterations = self
            .rest
            .get_json(&self.repo_url(&format!("pullRequests/{number}/iterations")))
            .await?;
        let last = iterations["value"]
            .as_array()
            .and_then(|a| a.last())
            .with_context(|| format!("PR {number} has no iterations"))?;
        let iteration = last["id"].as_u64().context("iteration id")?;
        let head = last["sourceRefCommit"]["commitId"]
            .as_str()
            .context("iteration sourceRefCommit")?
            .to_string();
        let base = last["commonRefCommit"]["commitId"]
            .as_str()
            .or_else(|| last["targetRefCommit"]["commitId"].as_str())
            .context("iteration commonRefCommit")?
            .to_string();
        let changes = self
            .rest
            .get_json(&self.repo_url(&format!(
                "pullRequests/{number}/iterations/{iteration}/changes?$top=2000&$compareTo=0"
            )))
            .await?;
        let entries = changes["changeEntries"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut out = String::new();
        for entry in entries.iter().take(MAX_DIFF_FILES) {
            let item = &entry["item"];
            if item["gitObjectType"].as_str().is_some_and(|t| t != "blob") {
                continue;
            }
            let Some(path) = item["path"].as_str() else {
                continue;
            };
            let change = entry["changeType"].as_str().unwrap_or("edit");
            let old_path = entry["originalPath"].as_str().unwrap_or(path);
            let old = if change.contains("add") {
                String::new()
            } else {
                self.file_at(old_path, &base).await.unwrap_or_default()
            };
            let new = if change.contains("delete") {
                String::new()
            } else {
                self.file_at(path, &head).await?
            };
            out.push_str(&unified_diff(old_path, path, &old, &new, change)?);
        }
        if entries.len() > MAX_DIFF_FILES {
            warn!(
                "Azure DevOps PR {number}: diff truncated to {MAX_DIFF_FILES} of {} files",
                entries.len()
            );
        }
        Ok(out)
    }

    async fn fetch_merged_pulls(&self, since: Option<&str>) -> Result<Vec<MergedPr>> {
        let mut out = Vec::new();
        self.list_prs("completed", |pr| {
            let Some(closed) = pr["closedDate"].as_str() else {
                return true;
            };
            if let Some(s) = since {
                if http::ts_before(closed, s) {
                    return false;
                }
            }
            out.push(MergedPr {
                number: pr["pullRequestId"].as_u64().unwrap_or(0),
                title: pr["title"].as_str().unwrap_or("").to_string(),
                body: pr["description"].as_str().map(String::from),
                author: pr["createdBy"]["uniqueName"].as_str().map(String::from),
                labels: http::label_names(&pr["labels"]),
                merged_at: closed.to_string(),
                created_at: pr["creationDate"].as_str().unwrap_or("").to_string(),
            });
            true
        })
        .await?;
        Ok(out)
    }

    async fn fetch_review_decisions(&self) -> Result<HashMap<u64, Option<String>>> {
        Ok(self
            .active_prs()
            .await?
            .iter()
            .filter_map(|pr| {
                let d = decision_from_votes(pr)?;
                Some((pr["pullRequestId"].as_u64()?, Some(d.to_string())))
            })
            .collect())
    }

    async fn fetch_ci_statuses(&self) -> Result<HashMap<u64, Option<String>>> {
        let prs = self.active_prs().await?;
        let targets: Vec<u64> = prs
            .iter()
            .take(PER_PR_CAP)
            .filter_map(|pr| pr["pullRequestId"].as_u64())
            .collect();
        // Azure Pipelines build validation is a branch policy, not a PR
        // status: its result is read from the policy evaluations of the PR.
        let project_id = match self.repo_ids().await {
            Ok((_, p)) => Some(p),
            Err(e) => {
                warn!("Azure DevOps: build policies not read (no project id): {e:#}");
                None
            }
        };
        let project_id = project_id.as_deref();
        let jobs = targets.into_iter().map(|id| async move {
            let url = self.repo_url(&format!("pullRequests/{id}/statuses"));
            let statuses = self.rest.get_json(&url).await;
            let builds = match project_id {
                Some(p) => {
                    let artifact = format!("vstfs:///CodeReview/CodeReviewId/{p}/{id}");
                    let url = self.project_url(
                        &format!("policy/evaluations?artifactId={}", enc(&artifact)),
                        POLICY_API,
                    );
                    match self.rest.get_json(&url).await {
                        Ok(v) => v["value"].as_array().cloned().unwrap_or_default(),
                        Err(e) => {
                            warn!("Azure DevOps PR {id}: policy evaluations unavailable: {e:#}");
                            Vec::new()
                        }
                    }
                }
                None => Vec::new(),
            };
            (id, statuses, builds)
        });
        let results: Vec<_> = futures::stream::iter(jobs)
            .buffer_unordered(PER_PR_CONCURRENCY)
            .collect()
            .await;
        let mut map = HashMap::new();
        for (id, res, builds) in results {
            // One PR's failure must not drop every other PR's state.
            let statuses = match res {
                Ok(v) => v,
                Err(e) => {
                    warn!("Azure DevOps PR {id}: statuses unavailable: {e:#}");
                    Value::Null
                }
            };
            // Latest status per context (genre/name).
            let mut latest: HashMap<String, (u64, String)> = HashMap::new();
            for s in statuses["value"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                let ctx = format!(
                    "{}/{}",
                    s["context"]["genre"].as_str().unwrap_or(""),
                    s["context"]["name"].as_str().unwrap_or("")
                );
                let sid = s["id"].as_u64().unwrap_or(0);
                let state = s["state"].as_str().unwrap_or("").to_string();
                if latest.get(&ctx).is_none_or(|(prev, _)| sid > *prev) {
                    latest.insert(ctx, (sid, state));
                }
            }
            let mut buckets: Vec<&str> = latest
                .values()
                .filter_map(|(_, st)| http::ci_bucket(st))
                .collect();
            buckets.extend(
                builds
                    .iter()
                    .filter(|e| {
                        e["configuration"]["type"]["id"] == BUILD_POLICY_TYPE
                            && e["configuration"]["isEnabled"].as_bool() != Some(false)
                    })
                    .filter_map(|e| policy_bucket(e["status"].as_str().unwrap_or(""))),
            );
            let combined = if buckets.contains(&"failure") {
                Some("failure")
            } else if buckets.contains(&"pending") {
                Some("pending")
            } else if buckets.contains(&"success") {
                Some("success")
            } else {
                None
            };
            if let Some(c) = combined {
                map.insert(id, Some(c.to_string()));
            }
        }
        Ok(map)
    }

    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        self.rest.require_auth("Azure DevOps PR label")?;
        for l in labels {
            self.rest
                .send_json(
                    Method::POST,
                    &self.repo_url_v(&format!("pullRequests/{number}/labels"), LABELS_API),
                    &json!({ "name": l }),
                )
                .await?;
        }
        Ok(())
    }

    async fn remove_pr_label(&self, number: u64, label: &str) -> Result<()> {
        self.rest.require_auth("Azure DevOps PR unlabel")?;
        self.rest
            .delete(&self.repo_url_v(
                &format!("pullRequests/{number}/labels/{}", enc(label)),
                LABELS_API,
            ))
            .await
    }

    async fn add_pr_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        // Azure PRs have no assignee: the closest equivalent is an
        // (optional) reviewer.
        self.rest.require_auth("Azure DevOps PR reviewer")?;
        for name in assignees {
            let id = self.identity_id(name).await?;
            self.rest
                .send_json(
                    Method::PUT,
                    &self.repo_url(&format!("pullRequests/{number}/reviewers/{id}")),
                    &json!({ "vote": 0, "isRequired": false }),
                )
                .await?;
        }
        Ok(())
    }

    async fn comment_pr(&self, number: u64, body: &str) -> Result<()> {
        self.upsert_comment(Thread::Pull(number), body).await
    }

    async fn submit_review(
        &self,
        pr_number: u64,
        body: &str,
        comments: &[(String, u64, String)],
    ) -> Result<()> {
        self.rest.require_auth("Azure DevOps review")?;
        for (path, line, comment) in comments {
            let file = if path.starts_with('/') {
                path.clone()
            } else {
                format!("/{path}")
            };
            self.post_thread(
                pr_number,
                &ensure_comment_marker(comment, &self.marker),
                Some(json!({
                    "filePath": file,
                    "rightFileStart": {"line": line, "offset": 1},
                    "rightFileEnd": {"line": line, "offset": 1},
                })),
                THREAD_ACTIVE,
            )
            .await?;
        }
        self.post_thread(
            pr_number,
            &ensure_comment_marker(body, &self.marker),
            None,
            THREAD_CLOSED,
        )
        .await
    }

    async fn merge_pr(&self, number: u64, sha: Option<&str>, squash: bool) -> Result<MergeOutcome> {
        // Direct completion only, pinned to the scored head: Azure checks
        // `lastMergeSourceCommit` and refuses the completion when the
        // source branch moved. No auto-complete: it would merge a later
        // state wshm never evaluated.
        self.rest.require_auth("Azure DevOps complete PR")?;
        let sha = sha
            .filter(|s| !s.is_empty())
            .with_context(|| format!("Azure DevOps PR {number}: refused, no scored head"))?;
        let pr = self
            .rest
            .get_json_opt(&self.repo_url(&format!("pullrequests/{number}")))
            .await?
            .with_context(|| format!("Azure DevOps PR {number} not found"))?;
        if pr["status"] != "active" {
            anyhow::bail!("Azure DevOps PR {number} is not active ({})", pr["status"]);
        }
        if pr["isDraft"].as_bool() == Some(true) {
            anyhow::bail!("Azure DevOps PR {number} is a draft: not completed");
        }
        if pr["mergeStatus"] == "conflicts" {
            anyhow::bail!("Azure DevOps PR {number} has merge conflicts");
        }
        let live = pr["lastMergeSourceCommit"]["commitId"]
            .as_str()
            .unwrap_or("");
        if !live.eq_ignore_ascii_case(sha) {
            anyhow::bail!(
                "Azure DevOps PR {number}: refused, head moved since it was scored ({sha} → {live})"
            );
        }
        let strategy = if squash { "squash" } else { "noFastForward" };
        let resp = self
            .rest
            .send_json(
                Method::PATCH,
                &self.repo_url(&format!("pullrequests/{number}")),
                &json!({
                    "status": "completed",
                    "lastMergeSourceCommit": {"commitId": sha},
                    "completionOptions": {
                        "mergeStrategy": strategy,
                        "deleteSourceBranch": false,
                        "transitionWorkItems": false,
                    },
                }),
            )
            .await?;
        // The merge is asynchronous: the answer can still show the PR
        // active (merge in progress, or blocked by a policy).
        if resp["status"] == "completed" {
            info!("Azure DevOps PR {number}: completed ({strategy})");
            Ok(MergeOutcome::Merged)
        } else {
            info!(
                "Azure DevOps PR {number}: completion requested ({strategy}), not merged yet \
                 (status {}, mergeStatus {})",
                resp["status"], resp["mergeStatus"]
            );
            Ok(MergeOutcome::NotMerged)
        }
    }

    async fn fetch_pr_approvers(&self, number: u64) -> Result<Vec<String>> {
        // Votes: 10 approved, 5 approved with suggestions. Groups vote
        // through their members' votes and are never an approver
        // themselves; the author and wshm's own account never count.
        let pr = self
            .rest
            .get_json_opt(&self.repo_url(&format!("pullrequests/{number}")))
            .await?
            .with_context(|| format!("Azure DevOps PR {number} not found"))?;
        let author = pr["createdBy"]["uniqueName"].as_str().unwrap_or("");
        let author_id = pr["createdBy"]["id"].as_str().unwrap_or("");
        let me = self.me().await;
        let mut out = Vec::new();
        for r in pr["reviewers"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            if r["vote"].as_i64().unwrap_or(0) < 5 || r["isContainer"].as_bool() == Some(true) {
                continue;
            }
            let Some(name) = r["uniqueName"].as_str().filter(|n| !n.is_empty()) else {
                continue;
            };
            let id = r["id"].as_str().unwrap_or("");
            if name.eq_ignore_ascii_case(author) || (!id.is_empty() && id == author_id) {
                continue;
            }
            if me
                .as_ref()
                .is_some_and(|(mid, mname)| mid == id || mname.eq_ignore_ascii_case(name))
            {
                continue;
            }
            match self.is_collaborator(name).await {
                Ok(true) => out.push(name.to_string()),
                Ok(false) => {
                    info!("Azure DevOps PR {number}: approval by {name} ignored (no write access)")
                }
                Err(e) => warn!("Azure DevOps PR {number}: approval by {name} ignored: {e:#}"),
            }
        }
        Ok(out)
    }

    async fn fetch_comment(&self, comment: &CommentRef) -> Result<Option<PolledComment>> {
        match comment.thread {
            Thread::Pull(n) => {
                let tid = comment.parent_id.context("PR comment without thread id")?;
                let Some(c) = self
                    .rest
                    .get_json_opt(&self.repo_url(&format!(
                        "pullRequests/{n}/threads/{tid}/comments/{}",
                        comment.id
                    )))
                    .await?
                else {
                    return Ok(None);
                };
                if c["isDeleted"].as_bool() == Some(true) || c["commentType"] == "system" {
                    return Ok(None);
                }
                Ok(Some(PolledComment {
                    id: comment.id,
                    number: n,
                    is_pr: true,
                    author: c["author"]["uniqueName"].as_str().unwrap_or("").to_string(),
                    body: c["content"].as_str().unwrap_or("").to_string(),
                    created_at: c["publishedDate"].as_str().unwrap_or("").to_string(),
                }))
            }
            Thread::Issue(n) => {
                let Some(c) = self
                    .rest
                    .get_json_opt(&self.project_url(
                        &format!("wit/workItems/{n}/comments/{}", comment.id),
                        COMMENTS_API,
                    ))
                    .await?
                else {
                    return Ok(None);
                };
                // Ids are organization-wide: the comment must be on this
                // work item, and the work item in this project.
                if c["isDeleted"].as_bool() == Some(true)
                    || c["workItemId"].as_u64().is_some_and(|w| w != n)
                    || self.work_item(n).await?.is_none()
                {
                    return Ok(None);
                }
                Ok(Some(PolledComment {
                    id: comment.id,
                    number: n,
                    is_pr: false,
                    author: c["createdBy"]["uniqueName"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    body: html_to_text(c["text"].as_str().unwrap_or("")),
                    created_at: c["createdDate"].as_str().unwrap_or("").to_string(),
                }))
            }
        }
    }

    async fn create_pr(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
        draft: bool,
    ) -> Result<u64> {
        self.rest.require_auth("Azure DevOps create PR")?;
        let resp = self
            .rest
            .send_json(
                Method::POST,
                &self.repo_url("pullrequests"),
                &json!({
                    "sourceRefName": format!("refs/heads/{head}"),
                    "targetRefName": format!("refs/heads/{base}"),
                    "title": title,
                    "description": body,
                    "isDraft": draft,
                }),
            )
            .await?;
        resp["pullRequestId"]
            .as_u64()
            .context("pullRequestId missing")
    }

    async fn find_comment_with_marker(
        &self,
        thread: Thread,
        marker: &str,
    ) -> Result<Option<CommentRef>> {
        match thread {
            Thread::Pull(n) => {
                for t in self.threads(n).await? {
                    if t["isDeleted"].as_bool() == Some(true) {
                        continue;
                    }
                    for c in t["comments"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                    {
                        if c["isDeleted"].as_bool() == Some(true) {
                            continue;
                        }
                        if http::has_marker(c["content"].as_str().unwrap_or(""), marker) {
                            return Ok(Some(CommentRef {
                                thread,
                                id: c["id"].as_u64().unwrap_or(0),
                                parent_id: t["id"].as_u64(),
                            }));
                        }
                    }
                }
                Ok(None)
            }
            Thread::Issue(n) => {
                // Paginated with continuationToken (200 per page max).
                let mut token: Option<String> = None;
                for _ in 0..http::MAX_PAGES {
                    let mut path = format!("wit/workItems/{n}/comments?$top=200&order=asc");
                    if let Some(t) = &token {
                        path.push_str(&format!("&continuationToken={}", enc(t)));
                    }
                    let resp = self
                        .rest
                        .get_json(&self.project_url(&path, COMMENTS_API))
                        .await?;
                    let found = resp["comments"]
                        .as_array()
                        .and_then(|a| {
                            a.iter().find(|c| {
                                c["isDeleted"].as_bool() != Some(true)
                                    && http::has_marker(c["text"].as_str().unwrap_or(""), marker)
                            })
                        })
                        .and_then(|c| c["id"].as_u64());
                    if let Some(id) = found {
                        return Ok(Some(CommentRef {
                            thread,
                            id,
                            parent_id: None,
                        }));
                    }
                    match resp["continuationToken"].as_str().filter(|t| !t.is_empty()) {
                        Some(t) => token = Some(t.to_string()),
                        None => break,
                    }
                }
                Ok(None)
            }
        }
    }

    async fn update_comment(&self, comment: &CommentRef, body: &str) -> Result<()> {
        self.rest.require_auth("Azure DevOps update comment")?;
        match comment.thread {
            Thread::Pull(n) => {
                let tid = comment.parent_id.context("PR comment without thread id")?;
                self.rest
                    .send_json(
                        Method::PATCH,
                        &self.repo_url(&format!(
                            "pullRequests/{n}/threads/{tid}/comments/{}",
                            comment.id
                        )),
                        &json!({ "content": body }),
                    )
                    .await?;
            }
            Thread::Issue(n) => {
                self.rest
                    .send_json(
                        Method::PATCH,
                        &self.project_url(
                            &format!("wit/workItems/{n}/comments/{}?format=markdown", comment.id),
                            COMMENTS_API,
                        ),
                        &json!({ "text": body }),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn delete_comment(&self, comment: &CommentRef) -> Result<()> {
        self.rest.require_auth("Azure DevOps delete comment")?;
        let url = match comment.thread {
            Thread::Pull(n) => {
                let tid = comment.parent_id.context("PR comment without thread id")?;
                self.repo_url(&format!(
                    "pullRequests/{n}/threads/{tid}/comments/{}",
                    comment.id
                ))
            }
            Thread::Issue(n) => self.project_url(
                &format!("wit/workItems/{n}/comments/{}", comment.id),
                COMMENTS_API,
            ),
        };
        self.rest.delete(&url).await
    }

    async fn is_collaborator(&self, username: &str) -> Result<bool> {
        // "Collaborator" = may push (Git "Contribute") to this repository,
        // evaluated from the access control lists of the repository and
        // of its parents (`repoV2/{project}`, `repoV2`) for the user and
        // every group it belongs to (nested, from the identity's
        // `memberOf`): a Contribute deny at any level wins, otherwise an
        // allow at a level the repository inherits from grants. Anything
        // that cannot be fully evaluated denies.
        if username.is_empty() || username.contains(['?', '&', '#']) {
            anyhow::bail!("Invalid Azure DevOps user name: {username}");
        }
        let Some(user) = self.find_identity(username, "ExpandedUp").await? else {
            info!("Azure DevOps: no identity named exactly '{username}' — denied");
            return Ok(false);
        };
        let Some(descriptor) = descriptor_str(&user["descriptor"]) else {
            return Ok(false);
        };
        let Some(member_of) = user["memberOf"].as_array() else {
            info!("Azure DevOps: group membership of {username} unknown — denied");
            return Ok(false);
        };
        let mut subjects = vec![descriptor];
        for m in member_of {
            match descriptor_str(m) {
                Some(d) => subjects.push(d),
                None => {
                    info!("Azure DevOps: unreadable group of {username} — denied");
                    return Ok(false);
                }
            }
        }
        let (repo_id, project_id) = self.repo_ids().await?;
        let tokens = [
            format!("repoV2/{project_id}/{repo_id}"),
            format!("repoV2/{project_id}"),
            "repoV2".to_string(),
        ];
        let (mut denied, mut allowed, mut inherits) = (false, false, true);
        for token in &tokens {
            let url = self.org_url(
                &format!(
                    "accesscontrollists/{GIT_NAMESPACE}?token={}&includeExtendedInfo=true&recurse=false",
                    enc(token)
                ),
                API,
            );
            let (acls, headers) = self.rest.get_json_with_headers(&url).await?;
            if headers.contains_key("x-ms-continuationtoken") {
                info!("Azure DevOps: paged access control list for {token} — denied");
                return Ok(false);
            }
            let Some(list) = acls["value"].as_array() else {
                info!("Azure DevOps: unreadable access control list for {token} — denied");
                return Ok(false);
            };
            let level_inherits = inherits;
            for acl in list {
                if acl["token"]
                    .as_str()
                    .is_some_and(|t| !t.eq_ignore_ascii_case(token))
                {
                    continue;
                }
                if acl["inheritPermissions"].as_bool() == Some(false) {
                    inherits = false;
                }
                let Some(aces) = acl["acesDictionary"].as_object() else {
                    continue;
                };
                for (key, ace) in aces {
                    let who = ace["descriptor"].as_str().unwrap_or(key);
                    if !subjects.iter().any(|d| d.eq_ignore_ascii_case(who)) {
                        continue;
                    }
                    let bits = |effective: &str, explicit: &str| {
                        ace[explicit].as_u64().unwrap_or(0)
                            | ace["extendedInfo"][effective].as_u64().unwrap_or(0)
                    };
                    if bits("effectiveDeny", "deny") & GENERIC_CONTRIBUTE != 0 {
                        denied = true;
                    }
                    if level_inherits && bits("effectiveAllow", "allow") & GENERIC_CONTRIBUTE != 0 {
                        allowed = true;
                    }
                }
            }
        }
        if denied {
            info!(
                "Azure DevOps: Contribute denied on {} for {username} — not a collaborator",
                self.repo
            );
            return Ok(false);
        }
        if !allowed {
            info!(
                "Azure DevOps: {username} has no Contribute right on {} — denied",
                self.repo
            );
        }
        Ok(allowed)
    }
}

/// Unified diff of two text blobs, `git diff` style, via libgit2.
fn unified_diff(
    old_path: &str,
    new_path: &str,
    old: &str,
    new: &str,
    change: &str,
) -> Result<String> {
    let old_p = old_path.trim_start_matches('/');
    let new_p = new_path.trim_start_matches('/');
    let mut opts = git2::DiffOptions::new();
    opts.context_lines(3);
    let mut patch = git2::Patch::from_buffers(
        old.as_bytes(),
        (!change.contains("add")).then_some(std::path::Path::new(old_p)),
        new.as_bytes(),
        (!change.contains("delete")).then_some(std::path::Path::new(new_p)),
        Some(&mut opts),
    )
    .context("libgit2 diff")?;
    let buf = patch.to_buf().context("libgit2 patch to text")?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_provider::mock::{MockServer, Reply};

    const R: &str = "/acme/Proj/_apis/git/repositories/web";

    fn provider(server: &MockServer) -> AzureDevOpsProvider {
        AzureDevOpsProvider::with_rest(
            Rest::new("Azure DevOps", Auth::BasicPassword("pat".into())).unwrap(),
            &server.url(),
            "acme",
            "Proj/web",
            "<!-- wshm -->".into(),
        )
    }

    #[tokio::test]
    async fn work_items_via_wiql_and_batch() {
        let server = MockServer::start().await;
        server.on(
            "POST",
            "/acme/Proj/_apis/wit/wiql",
            Reply::json(json!({"workItems": [{"id": 3}, {"id": 4}]})),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitems",
            Reply::json(json!({"value": [
                {"id": 3, "fields": {"System.Title": "a", "System.State": "Active",
                  "System.Tags": "bug; p1", "System.CreatedBy": {"uniqueName": "al@x.io"}}},
                {"id": 4, "fields": {"System.Title": "b", "System.State": "Done"}}
            ]})),
        );
        let issues = provider(&server)
            .fetch_all_issues(Some("2026-05-01T00:00:00Z"))
            .await
            .unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].labels, vec!["bug", "p1"]);
        assert_eq!(issues[0].state, "open");
        assert_eq!(issues[0].author.as_deref(), Some("al@x.io"));
        assert_eq!(issues[1].state, "closed");
        let reqs = server.requests();
        assert_eq!(
            reqs[0].header("authorization").as_deref(),
            Some("Basic OnBhdA==")
        );
        let q = reqs[0].json()["query"].as_str().unwrap().to_string();
        assert!(
            q.contains("[System.ChangedDate] >= '2026-05-01T00:00:00Z'"),
            "{q}"
        );
        assert!(reqs[1].query.contains("ids=3,4"));
    }

    #[tokio::test]
    async fn tags_are_merged_with_revision_guard() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitems/9",
            Reply::json(json!({"id": 9, "rev": 5, "fields": {"System.Tags": "bug", "System.TeamProject": "Proj"}})),
        );
        server.on(
            "PATCH",
            "/acme/Proj/_apis/wit/workitems/9",
            Reply::json(json!({"id": 9})),
        );
        let p = provider(&server);
        p.label_issue(9, &["Bug".into(), "triaged".into()])
            .await
            .unwrap();
        let reqs = server.requests();
        let patch = reqs.iter().find(|r| r.method == "PATCH").unwrap();
        assert_eq!(
            patch.header("content-type").as_deref(),
            Some("application/json-patch+json")
        );
        assert_eq!(
            patch.json(),
            json!([{"op": "test", "path": "/rev", "value": 5},
                   {"op": "add", "path": "/fields/System.Tags", "value": "bug; triaged"}])
        );
    }

    #[tokio::test]
    async fn work_item_of_other_project_is_ignored() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitems/9",
            Reply::json(json!({"id": 9, "fields": {"System.TeamProject": "Other"}})),
        );
        assert!(provider(&server).fetch_issue(9).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn pr_comment_upsert_uses_threads() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pullRequests/5/threads"),
            Reply::json(json!({"value": [
                {"id": 70, "comments": [{"id": 1, "content": "first"}]},
                {"id": 71, "comments": [{"id": 2, "content": "x <!-- wshm -->"}]}
            ]})),
        );
        server.on(
            "PATCH",
            &format!("{R}/pullRequests/5/threads/71/comments/2"),
            Reply::json(json!({})),
        );
        provider(&server).comment_pr(5, "body").await.unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[1].method, "PATCH");
        assert_eq!(reqs[1].json(), json!({"content": "body\n\n<!-- wshm -->"}));
    }

    #[tokio::test]
    async fn review_posts_inline_threads_then_summary() {
        let server = MockServer::start().await;
        server.on(
            "POST",
            &format!("{R}/pullRequests/5/threads"),
            Reply::json(json!({"id": 1})),
        );
        provider(&server)
            .submit_review(5, "sum", &[("src/a.rs".into(), 8, "nit".into())])
            .await
            .unwrap();
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2);
        let inline = reqs[0].json();
        assert_eq!(inline["threadContext"]["filePath"], "/src/a.rs");
        assert_eq!(inline["threadContext"]["rightFileStart"]["line"], 8);
        assert_eq!(inline["threadContext"]["rightFileEnd"]["line"], 8);
        // Findings are active threads; the summary is closed so it never
        // blocks a "comments must be resolved" policy.
        assert_eq!(inline["status"], THREAD_ACTIVE);
        assert_eq!(reqs[1].json()["status"], THREAD_CLOSED);
        assert_eq!(inline["comments"][0]["content"], "nit\n\n<!-- wshm -->");
        assert_eq!(
            reqs[1].json()["comments"][0]["content"],
            "sum\n\n<!-- wshm -->"
        );
        assert!(reqs[1].json().get("threadContext").is_none());
    }

    #[tokio::test]
    async fn pr_mapping_votes_and_labels() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pullrequests"),
            Reply::json(json!({"value": [
                {"pullRequestId": 5, "title": "t", "status": "active", "creationDate": "2026-05-01T00:00:00Z",
                 "sourceRefName": "refs/heads/feat", "targetRefName": "refs/heads/main",
                 "lastMergeSourceCommit": {"commitId": "h"}, "mergeStatus": "conflicts",
                 "reviewers": [{"vote": 10}, {"vote": -5}], "labels": [{"name": "x"}]},
                {"pullRequestId": 6, "status": "active", "reviewers": [{"vote": 5}]},
                {"pullRequestId": 7, "status": "active", "reviewers": [{"vote": 0}]}
            ]})),
        );
        server.on(
            "POST",
            &format!("{R}/pullRequests/5/labels"),
            Reply::json(json!({})),
        );
        let p = provider(&server);
        let prs = p.fetch_pulls_incremental("open", None).await.unwrap();
        assert_eq!(prs[0].head_ref.as_deref(), Some("feat"));
        assert_eq!(prs[0].mergeable, Some(false));
        assert_eq!(prs[0].labels, vec!["x"]);
        let d = p.fetch_review_decisions().await.unwrap();
        assert_eq!(d[&5].as_deref(), Some("changes_requested"));
        assert_eq!(d[&6].as_deref(), Some("approved"));
        assert_eq!(d[&7].as_deref(), Some("review_required"));
        p.label_pr(5, &["risk:high".into()]).await.unwrap();
        let label = server
            .requests()
            .into_iter()
            .find(|r| r.method == "POST")
            .unwrap();
        assert!(label.query.contains("api-version=7.1-preview.1"));
        assert_eq!(label.json(), json!({"name": "risk:high"}));
    }

    #[tokio::test]
    async fn diff_is_rebuilt_from_iteration_changes() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pullRequests/5/iterations"),
            Reply::json(
                json!({"value": [{"id": 2, "sourceRefCommit": {"commitId": "head"},
                                          "commonRefCommit": {"commitId": "base"}}]}),
            ),
        );
        server.on(
            "GET",
            &format!("{R}/pullRequests/5/iterations/2/changes"),
            Reply::json(json!({"changeEntries": [
                {"changeType": "edit", "item": {"path": "/src/a.rs", "gitObjectType": "blob"}},
                {"changeType": "add", "item": {"path": "/new.txt", "gitObjectType": "blob"}}
            ]})),
        );
        server.on(
            "GET",
            &format!("{R}/items"),
            Reply::text("one\ntwo\n").when_query("version=base"),
        );
        server.on(
            "GET",
            &format!("{R}/items"),
            Reply::text("one\n2\n").when_query("version=head"),
        );
        let diff = provider(&server).fetch_pr_diff(5).await.unwrap();
        assert!(diff.contains("diff --git a/src/a.rs b/src/a.rs"), "{diff}");
        assert!(diff.contains("-two\n+2\n"), "{diff}");
        assert!(diff.contains("+++ b/new.txt"), "{diff}");
    }

    #[tokio::test]
    async fn work_item_types_filter_with_db_override() {
        let server = MockServer::start().await;
        server.on(
            "POST",
            "/acme/Proj/_apis/wit/wiql",
            Reply::json(json!({"workItems": []})),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitems/9",
            Reply::json(json!({"id": 9, "fields": {"System.WorkItemType": "Task", "System.TeamProject": "Proj"}})),
        );
        let mut p = provider(&server);
        p.fetch_all_issues(None).await.unwrap();
        // A Task is not an issue.
        assert!(p.fetch_issue(9).await.unwrap().is_none());

        let db = Arc::new(crate::Database::open_memory().unwrap());
        db.set_app_setting(
            crate::db::settings::AZURE_WORK_ITEM_TYPES_KEY,
            r#"["User Story","O'Hara"]"#,
        )
        .unwrap();
        p.db = Some(db);
        p.fetch_all_issues(None).await.unwrap();
        let queries: Vec<String> = server
            .requests()
            .iter()
            .filter(|r| r.method == "POST")
            .map(|r| r.json()["query"].as_str().unwrap().to_string())
            .collect();
        assert!(
            queries[0].contains("[System.WorkItemType] IN ('Issue', 'Bug')"),
            "{}",
            queries[0]
        );
        assert!(
            queries[1].contains("IN ('User Story', 'O''Hara')"),
            "{}",
            queries[1]
        );
    }

    #[tokio::test]
    async fn work_item_comment_lookup_follows_continuation() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workItems/3/comments",
            Reply::json(json!({"comments": [{"id": 1, "text": "hi"}], "continuationToken": "abc"}))
                .when_query("order=asc&api"),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workItems/3/comments",
            Reply::json(json!({"comments": [{"id": 2, "text": "x <!-- wshm -->"}]}))
                .when_query("continuationToken=abc"),
        );
        let found = provider(&server)
            .find_comment_with_marker(Thread::Issue(3), "<!-- wshm -->")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, 2);
    }

    #[test]
    fn mermaid_not_claimed() {
        let p = AzureDevOpsProvider::with_rest(
            Rest::new("Azure DevOps", Auth::None).unwrap(),
            "https://dev.azure.com",
            "o",
            "p",
            "m".into(),
        );
        assert!(!p.supports_mermaid());
        assert!(!p.supports(Capability::CommentPolling));
        assert_eq!(p.repo_slug(), "o/p");
    }

    #[tokio::test]
    async fn wiql_pages_by_id_past_the_query_cap() {
        let server = MockServer::start().await;
        // Page 2 (keyset after id 8) first: rules match in order.
        server.on(
            "POST",
            "/acme/Proj/_apis/wit/wiql",
            Reply::json(json!({"workItems": [{"id": 5}]})).when_body("[System.Id] < 8"),
        );
        server.on(
            "POST",
            "/acme/Proj/_apis/wit/wiql",
            Reply::json(json!({"workItems": [{"id": 9}, {"id": 8}]})),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitems",
            Reply::json(
                json!({"value": [{"id": 9, "fields": {}}, {"id": 8, "fields": {}},
                                          {"id": 5, "fields": {}}]}),
            ),
        );
        let mut p = provider(&server);
        p.wiql_page = 2;
        let issues = p.fetch_issues(None).await.unwrap();
        assert_eq!(issues.len(), 3);
        let reqs = server.requests();
        let wiql: Vec<String> = reqs
            .iter()
            .filter(|r| r.method == "POST")
            .map(|r| r.json()["query"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(wiql.len(), 2, "{wiql:?}");
        assert!(wiql[0].contains("ORDER BY [System.Id] DESC"), "{}", wiql[0]);
        assert!(!wiql[0].contains("[System.Id] <"));
        assert!(wiql[1].contains("AND [System.Id] < 8"), "{}", wiql[1]);
        assert!(reqs.iter().any(|r| r.query.contains("$top=2")));
        let batch = reqs.iter().find(|r| r.method == "GET").unwrap();
        assert!(batch.query.contains("ids=9,8,5"), "{}", batch.query);
    }

    #[tokio::test]
    async fn tag_removal_patches_the_remaining_tags() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitems/9",
            Reply::json(json!({"id": 9, "rev": 2, "fields": {
                "System.Tags": "bug; wshm:triaged; p1", "System.TeamProject": "Proj"}})),
        );
        server.on(
            "PATCH",
            "/acme/Proj/_apis/wit/workitems/9",
            Reply::json(json!({"id": 9})),
        );
        let p = provider(&server);
        p.remove_label(9, "WSHM:Triaged").await.unwrap();
        // Absent tag: no write at all.
        p.remove_label(9, "nope").await.unwrap();
        let patches: Vec<_> = server
            .requests()
            .into_iter()
            .filter(|r| r.method == "PATCH")
            .collect();
        assert_eq!(patches.len(), 1);
        assert_eq!(
            patches[0].json(),
            json!([{"op": "test", "path": "/rev", "value": 2},
                   {"op": "add", "path": "/fields/System.Tags", "value": "bug; p1"}])
        );
    }

    #[tokio::test]
    async fn work_item_comment_created_then_updated_by_marker() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workItems/3/comments",
            Reply::json(json!({"comments": [{"id": 1, "text": "hi"}]})),
        );
        server.on(
            "POST",
            "/acme/Proj/_apis/wit/workItems/3/comments",
            Reply::json(json!({"id": 2})),
        );
        let p = provider(&server);
        p.comment_issue(3, "triaged").await.unwrap();
        let reqs = server.requests();
        let post = reqs.iter().find(|r| r.method == "POST").unwrap();
        assert!(post.query.contains("format=markdown"));
        assert!(post.query.contains("api-version=7.1-preview.4"));
        assert_eq!(post.json(), json!({"text": "triaged\n\n<!-- wshm -->"}));

        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workItems/3/comments",
            Reply::json(json!({"comments": [
                {"id": 1, "text": "old <!-- wshm -->", "isDeleted": true},
                {"id": 4, "text": "triaged <!-- wshm -->"}]})),
        );
        server.on(
            "PATCH",
            "/acme/Proj/_apis/wit/workItems/3/comments/4",
            Reply::json(json!({"id": 4})),
        );
        provider(&server).comment_issue(3, "again").await.unwrap();
        let reqs = server.requests();
        assert!(!reqs.iter().any(|r| r.method == "POST"));
        let patch = reqs.iter().find(|r| r.method == "PATCH").unwrap();
        assert_eq!(patch.json(), json!({"text": "again\n\n<!-- wshm -->"}));
    }

    #[tokio::test]
    async fn new_pr_comment_is_a_closed_thread() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pullRequests/5/threads"),
            Reply::json(json!({"value": [
                {"id": 70, "isDeleted": true, "comments": [{"id": 1, "content": "<!-- wshm -->"}]}
            ]})),
        );
        server.on(
            "POST",
            &format!("{R}/pullRequests/5/threads"),
            Reply::json(json!({"id": 71})),
        );
        provider(&server).comment_pr(5, "analysis").await.unwrap();
        let post = server
            .requests()
            .into_iter()
            .find(|r| r.method == "POST")
            .unwrap();
        assert_eq!(post.json()["status"], THREAD_CLOSED);
        assert_eq!(
            post.json()["comments"][0]["content"],
            "analysis\n\n<!-- wshm -->"
        );
        assert!(post.json().get("threadContext").is_none());
    }

    fn connection_data(server: &MockServer) {
        server.on(
            "GET",
            "/acme/_apis/connectionData",
            Reply::json(json!({"authenticatedUser": {"id": "me-1",
                "properties": {"Account": {"$value": "wshm-bot@x.io"}}}})),
        );
    }

    #[tokio::test]
    async fn merge_status_maps_to_mergeable() {
        for (status, expected) in [
            ("succeeded", Some(true)),
            ("conflicts", Some(false)),
            ("queued", None),
            ("rejectedByPolicy", None),
            ("failure", None),
        ] {
            let server = MockServer::start().await;
            server.on(
                "GET",
                &format!("{R}/pullrequests/5"),
                Reply::json(json!({"pullRequestId": 5, "status": "active", "mergeStatus": status})),
            );
            let got = provider(&server).fetch_pr_mergeable(5).await.unwrap();
            assert_eq!(got, expected, "{status}");
        }
    }

    #[tokio::test]
    async fn ci_reads_build_policies_and_statuses() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pullrequests"),
            Reply::json(json!({"value": [{"pullRequestId": 5, "status": "active"},
                                          {"pullRequestId": 6, "status": "active"},
                                          {"pullRequestId": 7, "status": "active"}]})),
        );
        server.on(
            "GET",
            R,
            Reply::json(json!({"id": "r1", "project": {"id": "p1"}})),
        );
        for id in [5, 6, 7] {
            server.on(
                "GET",
                &format!("{R}/pullRequests/{id}/statuses"),
                Reply::json(json!({"value": if id == 7 {
                    json!([{"id": 1, "state": "succeeded", "context": {"genre": "g", "name": "lint"}}])
                } else { json!([]) }})),
            );
        }
        let build = |status: &str| {
            json!({"status": status, "configuration": {"isEnabled": true,
                   "type": {"id": BUILD_POLICY_TYPE}}})
        };
        let reviewers = json!({"status": "rejected", "configuration": {"isEnabled": true,
                               "type": {"id": "fa4e907d-c16b-4a4c-9dfa-4906e5d171dd"}}});
        server.on(
            "GET",
            "/acme/Proj/_apis/policy/evaluations",
            Reply::json(json!({"value": [build("rejected")]})).when_query("p1%2F5"),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/policy/evaluations",
            Reply::json(json!({"value": [build("running"), reviewers]})).when_query("p1%2F6"),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/policy/evaluations",
            Reply::json(json!({"value": [build("approved")]})).when_query("p1%2F7"),
        );
        let ci = provider(&server).fetch_ci_statuses().await.unwrap();
        assert_eq!(ci[&5].as_deref(), Some("failure"));
        // A rejected reviewer policy is not CI.
        assert_eq!(ci[&6].as_deref(), Some("pending"));
        assert_eq!(ci[&7].as_deref(), Some("success"));
        let eval = server
            .requests()
            .into_iter()
            .find(|r| r.path.ends_with("policy/evaluations"))
            .unwrap();
        assert!(
            eval.query
                .contains("artifactId=vstfs%3A%2F%2F%2FCodeReview%2FCodeReviewId%2Fp1%2F"),
            "{}",
            eval.query
        );
        assert!(eval.query.contains("api-version=7.1-preview.1"));
    }

    #[tokio::test]
    async fn duplicate_close_prefers_the_removed_category() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitems/9",
            Reply::json(json!({"id": 9, "fields": {"System.WorkItemType": "Bug",
                "System.State": "Active", "System.TeamProject": "Proj"}})),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitemtypes/Bug/states",
            Reply::json(json!({"value": [
                {"name": "New", "category": "Proposed"},
                {"name": "Closed", "category": "Completed"},
                {"name": "Removed", "category": "Removed"}]})),
        );
        server.on(
            "PATCH",
            "/acme/Proj/_apis/wit/workitems/9",
            Reply::json(json!({"id": 9})),
        );
        let p = provider(&server);
        p.close_issue_not_planned(9).await.unwrap();
        p.close_issue(9).await.unwrap();
        let states: Vec<Value> = server
            .requests()
            .into_iter()
            .filter(|r| r.method == "PATCH")
            .map(|r| r.json()[0]["value"].clone())
            .collect();
        assert_eq!(states, vec![json!("Removed"), json!("Closed")]);
    }

    #[tokio::test]
    async fn sign_in_page_is_an_auth_error() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pullrequests/5"),
            Reply::text("<html>Sign in</html>").status(203),
        );
        let err = provider(&server).fetch_pull(5).await.unwrap_err();
        assert!(format!("{err:#}").contains("203"), "{err:#}");
    }

    #[tokio::test]
    async fn project_visibility() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/_apis/projects/Proj",
            Reply::json(json!({"visibility": "public"})),
        );
        assert!(!provider(&server).fetch_repo_private().await.unwrap());
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/_apis/projects/Proj",
            Reply::json(json!({"visibility": "private"})),
        );
        assert!(provider(&server).fetch_repo_private().await.unwrap());
    }

    fn active_pr(server: &MockServer, extra: Value) {
        let mut pr = json!({"pullRequestId": 5, "status": "active", "mergeStatus": "succeeded",
                            "lastMergeSourceCommit": {"commitId": "abc"}});
        for (k, v) in extra.as_object().unwrap() {
            pr[k] = v.clone();
        }
        server.on("GET", &format!("{R}/pullrequests/5"), Reply::json(pr));
    }

    #[tokio::test]
    async fn merge_completes_pinned_to_the_scored_head() {
        let server = MockServer::start().await;
        active_pr(&server, json!({}));
        server.on(
            "PATCH",
            &format!("{R}/pullrequests/5"),
            Reply::json(json!({"pullRequestId": 5, "status": "completed"})),
        );
        let p = provider(&server);
        assert!(p.supports(Capability::Merge));
        assert_eq!(
            p.merge_pr(5, Some("abc"), true).await.unwrap(),
            MergeOutcome::Merged
        );
        let patch = server
            .requests()
            .into_iter()
            .find(|r| r.method == "PATCH")
            .unwrap();
        // Direct completion with the scored head (Azure checks it), never
        // auto-complete.
        assert_eq!(
            patch.json(),
            json!({"status": "completed", "lastMergeSourceCommit": {"commitId": "abc"},
                   "completionOptions": {"mergeStrategy": "squash", "deleteSourceBranch": false,
                                         "transitionWorkItems": false}})
        );
        assert!(patch.json().get("autoCompleteSetBy").is_none());

        // Still active in the answer (asynchronous merge, or a policy
        // blocks it): not merged; merge commit strategy.
        let server = MockServer::start().await;
        active_pr(&server, json!({}));
        server.on(
            "PATCH",
            &format!("{R}/pullrequests/5"),
            Reply::json(json!({"pullRequestId": 5, "status": "active"})),
        );
        assert_eq!(
            provider(&server)
                .merge_pr(5, Some("abc"), false)
                .await
                .unwrap(),
            MergeOutcome::NotMerged
        );
        let patch = server
            .requests()
            .into_iter()
            .find(|r| r.method == "PATCH")
            .unwrap();
        assert_eq!(
            patch.json()["completionOptions"]["mergeStrategy"],
            "noFastForward"
        );
    }

    #[tokio::test]
    async fn merge_refuses_moved_heads_and_unsafe_states() {
        for (extra, sha, why) in [
            (
                json!({"lastMergeSourceCommit": {"commitId": "xyz"}}),
                Some("abc"),
                "head moved",
            ),
            (
                json!({"mergeStatus": "conflicts"}),
                Some("abc"),
                "conflicts",
            ),
            (json!({"isDraft": true}), Some("abc"), "draft"),
            (json!({"status": "completed"}), Some("abc"), "not active"),
            (json!({}), None, "no scored head"),
        ] {
            let server = MockServer::start().await;
            active_pr(&server, extra);
            let err = provider(&server).merge_pr(5, sha, false).await.unwrap_err();
            assert!(format!("{err:#}").contains(why), "{err:#}");
            assert!(!server.requests().iter().any(|r| r.method == "PATCH"));
        }
        // Anonymous: refused before any request.
        let server = MockServer::start().await;
        let anon = AzureDevOpsProvider::with_rest(
            Rest::new("Azure DevOps", Auth::None).unwrap(),
            &server.url(),
            "acme",
            "Proj/web",
            "<!-- wshm -->".into(),
        );
        assert!(anon.merge_pr(5, Some("abc"), false).await.is_err());
        assert!(server.requests().is_empty());
    }

    /// An identity found by exact account name, with its groups.
    fn identity(server: &MockServer, account: &str, member_of: Value) {
        server.on(
            "GET",
            "/acme/_apis/identities",
            Reply::json(json!({"value": [{
                "id": format!("id-{account}"), "descriptor": format!("d-{account}"),
                "memberOf": member_of,
                "properties": {"Account": {"$value": account}}}]}))
            .when_query(&format!("filterValue={}", enc(account))),
        );
    }

    /// ACLs of the repository, project and root tokens, plus the repo ids.
    fn acls(server: &MockServer, repo: Value, project: Value, root: Value) {
        server.on(
            "GET",
            R,
            Reply::json(json!({"id": "r1", "project": {"id": "p1"}})),
        );
        let path = format!("/acme/_apis/accesscontrollists/{GIT_NAMESPACE}");
        for (fragment, token, acl) in [
            ("token=repoV2%2Fp1%2Fr1&", "repoV2/p1/r1", repo),
            ("token=repoV2%2Fp1&", "repoV2/p1", project),
            ("token=repoV2&", "repoV2", root),
        ] {
            let mut acl = acl;
            acl["token"] = json!(token);
            server.on(
                "GET",
                &path,
                Reply::json(json!({"value": [acl]})).when_query(fragment),
            );
        }
    }

    fn aces(v: Value) -> Value {
        json!({"acesDictionary": v})
    }

    #[tokio::test]
    async fn collaborator_through_a_group_allow_on_a_parent_token() {
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!(["g-team", "g-contrib"]));
        acls(
            &server,
            aces(json!({})),
            aces(json!({"g-contrib": {"descriptor": "g-contrib", "allow": 6, "deny": 0}})),
            aces(json!({})),
        );
        assert!(provider(&server).is_collaborator("bob@x.io").await.unwrap());
        let reqs = server.requests();
        let user_q = reqs
            .iter()
            .find(|r| r.query.contains("bob%40x.io"))
            .unwrap();
        assert!(user_q.query.contains("queryMembership=ExpandedUp"));
        let acl_reqs: Vec<_> = reqs
            .iter()
            .filter(|r| r.path.contains("accesscontrollists"))
            .collect();
        // Repository, project and root tokens, all ACEs (no descriptor
        // filter: no URL length limit on large memberships).
        assert_eq!(acl_reqs.len(), 3);
        assert!(acl_reqs.iter().all(|r| !r.query.contains("descriptors=")));
        assert!(acl_reqs.iter().all(|r| r.query.contains("recurse=false")));
    }

    #[tokio::test]
    async fn collaborator_denies_win_at_any_level() {
        // A nested group of the user is denied at the root.
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!(["g-contrib", "g-nested"]));
        acls(
            &server,
            aces(json!({"d-bob@x.io": {"allow": 4, "deny": 0}})),
            aces(json!({"g-contrib": {"allow": 4, "deny": 0}})),
            aces(json!({"g-nested": {"allow": 0, "deny": 4}})),
        );
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());

        // Effective deny reported by the service for the user.
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!(["g-contrib"]));
        acls(
            &server,
            aces(json!({"d-bob@x.io": {"allow": 0, "deny": 0,
                                        "extendedInfo": {"effectiveDeny": 4}}})),
            aces(json!({"g-contrib": {"allow": 4, "deny": 0}})),
            aces(json!({})),
        );
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());

        // A deny on some other group of the project does not concern bob.
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!(["g-contrib"]));
        acls(
            &server,
            aces(json!({"g-other": {"allow": 0, "deny": 4}})),
            aces(json!({"g-contrib": {"allow": 4, "deny": 0}})),
            aces(json!({})),
        );
        assert!(provider(&server).is_collaborator("bob@x.io").await.unwrap());
    }

    #[tokio::test]
    async fn collaborator_allow_rules() {
        // Explicit allow for the user on the repository.
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!([]));
        acls(
            &server,
            aces(json!({"d-bob@x.io": {"allow": 6, "deny": 0}})),
            aces(json!({})),
            aces(json!({})),
        );
        assert!(provider(&server).is_collaborator("bob@x.io").await.unwrap());

        // Read only: denied.
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!(["g-readers"]));
        acls(
            &server,
            aces(json!({})),
            aces(json!({"g-readers": {"allow": 2, "deny": 0}})),
            aces(json!({})),
        );
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());

        // The repository does not inherit: a project-level allow is moot.
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!(["g-contrib"]));
        let mut repo = aces(json!({}));
        repo["inheritPermissions"] = json!(false);
        acls(
            &server,
            repo,
            aces(json!({"g-contrib": {"allow": 4, "deny": 0}})),
            aces(json!({})),
        );
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());
    }

    #[tokio::test]
    async fn collaborator_incomplete_evaluation_denies() {
        // Group membership unknown (no memberOf).
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/_apis/identities",
            Reply::json(json!({"value": [{"descriptor": "d-bob",
                "properties": {"Account": {"$value": "bob@x.io"}}}]})),
        );
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());
        assert!(!server
            .requests()
            .iter()
            .any(|r| r.path.contains("accesscontrollists")));

        // An unreadable group entry.
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!([42]));
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());

        // A paged access control list.
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!(["g-contrib"]));
        server.on(
            "GET",
            R,
            Reply::json(json!({"id": "r1", "project": {"id": "p1"}})),
        );
        server.on(
            "GET",
            &format!("/acme/_apis/accesscontrollists/{GIT_NAMESPACE}"),
            Reply::json(json!({"value": [{"token": "repoV2/p1/r1",
                "acesDictionary": {"g-contrib": {"allow": 4, "deny": 0}}}]}))
            .header("x-ms-continuationtoken", "next"),
        );
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());

        // An ACL call failing (here: the root token is not answered).
        let server = MockServer::start().await;
        identity(&server, "bob@x.io", json!(["g-contrib"]));
        server.on(
            "GET",
            R,
            Reply::json(json!({"id": "r1", "project": {"id": "p1"}})),
        );
        server.on(
            "GET",
            &format!("/acme/_apis/accesscontrollists/{GIT_NAMESPACE}"),
            Reply::json(json!({"value": [{"token": "repoV2/p1/r1",
                "acesDictionary": {"d-bob@x.io": {"allow": 4, "deny": 0}}}]}))
            .when_query("token=repoV2%2Fp1%2Fr1&"),
        );
        assert!(provider(&server).is_collaborator("bob@x.io").await.is_err());
    }

    #[tokio::test]
    async fn collaborator_needs_an_exact_identity() {
        // The identity search is fuzzy: "bob" also returns bobby.
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/_apis/identities",
            Reply::json(
                json!({"value": [{"descriptor": "d-bobby", "memberOf": ["g-contrib"],
                "properties": {"Account": {"$value": "bobby@x.io"}}}]}),
            ),
        );
        assert!(!provider(&server).is_collaborator("bob").await.unwrap());
        assert!(!server
            .requests()
            .iter()
            .any(|r| r.path.contains("accesscontrollists")));
        assert!(provider(&server).is_collaborator("a&b").await.is_err());
    }

    #[test]
    fn descriptors_as_strings_or_objects() {
        assert_eq!(descriptor_str(&json!("T;1")).as_deref(), Some("T;1"));
        assert_eq!(
            descriptor_str(&json!({"identityType": "T", "identifier": "1"})).as_deref(),
            Some("T;1")
        );
        assert_eq!(descriptor_str(&json!(null)), None);
    }

    #[tokio::test]
    async fn approvers_are_voters_with_write_access() {
        let server = MockServer::start().await;
        connection_data(&server);
        active_pr(
            &server,
            json!({
                "createdBy": {"id": "id-author", "uniqueName": "author@x.io"},
                "reviewers": [
                    {"id": "id-author", "uniqueName": "author@x.io", "vote": 10},
                    {"id": "me-1", "uniqueName": "wshm-bot@x.io", "vote": 10},
                    {"id": "grp", "uniqueName": "[Proj]\\Reviewers", "vote": 10, "isContainer": true},
                    {"id": "id-alice", "uniqueName": "alice@x.io", "vote": 10},
                    {"id": "id-carol", "uniqueName": "carol@x.io", "vote": 5},
                    {"id": "id-dave", "uniqueName": "dave@x.io", "vote": -5},
                    {"id": "id-erin", "uniqueName": "erin@x.io", "vote": 0},
                    {"id": "id-mallory", "uniqueName": "mallory@x.io", "vote": 10}
                ]
            }),
        );
        identity(&server, "alice@x.io", json!(["g-contrib"]));
        identity(&server, "carol@x.io", json!(["g-contrib"]));
        identity(&server, "mallory@x.io", json!(["g-readers"]));
        acls(
            &server,
            aces(json!({})),
            aces(json!({"g-contrib": {"allow": 4, "deny": 0},
                        "g-readers": {"allow": 2, "deny": 0}})),
            aces(json!({})),
        );
        let p = provider(&server);
        let approvers = p.fetch_pr_approvers(5).await.unwrap();
        assert_eq!(approvers, vec!["alice@x.io", "carol@x.io"]);
        // Neither the author nor wshm itself was even checked.
        let reqs = server.requests();
        assert!(!reqs.iter().any(|r| r.query.contains("author%40x.io")));
        assert!(!reqs.iter().any(|r| r.query.contains("wshm-bot%40x.io")));
    }

    #[tokio::test]
    async fn comments_are_read_back_from_the_api() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pullRequests/7/threads/41/comments/3"),
            Reply::json(
                json!({"id": 3, "content": "/wshm review", "commentType": "text",
                               "author": {"uniqueName": "bob@x.io"},
                               "publishedDate": "2026-05-01T00:00:00Z"}),
            ),
        );
        server.on(
            "GET",
            &format!("{R}/pullRequests/7/threads/41/comments/4"),
            Reply::json(json!({"id": 4, "content": "x", "isDeleted": true})),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workItems/42/comments/77",
            Reply::json(json!({"id": 77, "workItemId": 42,
                               "text": "<div>/wshm fix &amp; go</div>",
                               "createdBy": {"uniqueName": "carol@x.io"}})),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitems/42",
            Reply::json(json!({"id": 42, "fields": {"System.TeamProject": "Proj"}})),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workItems/43/comments/78",
            Reply::json(json!({"id": 78, "workItemId": 43, "text": "/wshm fix",
                               "createdBy": {"uniqueName": "eve@x.io"}})),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/wit/workitems/43",
            Reply::json(json!({"id": 43, "fields": {"System.TeamProject": "Other"}})),
        );
        let p = provider(&server);
        let pr = |id| CommentRef {
            thread: Thread::Pull(7),
            id,
            parent_id: Some(41),
        };
        let c = p.fetch_comment(&pr(3)).await.unwrap().unwrap();
        assert_eq!(
            (c.author.as_str(), c.body.as_str()),
            ("bob@x.io", "/wshm review")
        );
        assert!(c.is_pr);
        assert!(p.fetch_comment(&pr(4)).await.unwrap().is_none(), "deleted");
        assert!(p.fetch_comment(&pr(5)).await.unwrap().is_none(), "missing");
        let wi = |n, id| CommentRef {
            thread: Thread::Issue(n),
            id,
            parent_id: None,
        };
        let c = p.fetch_comment(&wi(42, 77)).await.unwrap().unwrap();
        assert_eq!(
            (c.author.as_str(), c.body.as_str()),
            ("carol@x.io", "/wshm fix & go")
        );
        // A work item of another project (ids are organization-wide).
        assert!(p.fetch_comment(&wi(43, 78)).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn ci_keeps_other_prs_when_one_fails() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pullrequests"),
            Reply::json(json!({"value": [{"pullRequestId": 5, "status": "active"},
                                          {"pullRequestId": 6, "status": "active"}]})),
        );
        server.on(
            "GET",
            R,
            Reply::json(json!({"id": "r1", "project": {"id": "p1"}})),
        );
        server.on(
            "GET",
            &format!("{R}/pullRequests/5/statuses"),
            Reply::json(json!({"value": [{"id": 1, "state": "failed",
                                          "context": {"genre": "g", "name": "lint"}}]})),
        );
        server.on(
            "GET",
            &format!("{R}/pullRequests/6/statuses"),
            Reply::json(json!({"message": "denied"})).status(403),
        );
        server.on(
            "GET",
            "/acme/Proj/_apis/policy/evaluations",
            Reply::json(json!({"value": []})),
        );
        let ci = provider(&server).fetch_ci_statuses().await.unwrap();
        assert_eq!(ci[&5].as_deref(), Some("failure"));
        assert!(!ci.contains_key(&6));
    }
}
