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
use super::{Capability, CommentRef, GitProvider, MergedPr, Thread};

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
/// Project groups whose members may push by default: the trust anchor of
/// [`GitProvider::is_collaborator`] when there is no explicit ACE.
const WRITER_GROUPS: [&str; 2] = ["Contributors", "Project Administrators"];
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

    /// Descriptor of a project group (`[Project]\Contributors`), exact name.
    async fn group_descriptor(&self, group: &str) -> Result<Option<String>> {
        let name = format!("[{}]\\{group}", self.project);
        let resp = self
            .rest
            .get_json(&self.identities_url(&name, "None"))
            .await?;
        Ok(resp["value"].as_array().and_then(|a| {
            a.iter()
                .find(|i| {
                    i["providerDisplayName"]
                        .as_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(&name))
                })
                .and_then(|i| descriptor_str(&i["descriptor"]))
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

/// Azure merge strategy for `[queue] strategy`.
fn merge_strategy(strategy: &str) -> Result<&'static str> {
    Ok(match strategy.trim().to_ascii_lowercase().as_str() {
        "merge" | "" => "noFastForward",
        "squash" => "squash",
        "rebase" => "rebase",
        "rebase-merge" | "rebasemerge" => "rebaseMerge",
        other => anyhow::bail!(
            "unknown [queue] strategy '{other}' for Azure DevOps (merge, squash, rebase, rebase-merge)"
        ),
    })
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
        }
    }

    fn supports_merge(&self) -> bool {
        true
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
            // Latest status per context (genre/name).
            let mut latest: HashMap<String, (u64, String)> = HashMap::new();
            for s in res?["value"]
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

    async fn merge_pr(&self, number: u64, strategy: &str) -> Result<()> {
        // Completion goes through auto-complete: Azure completes the PR as
        // soon as every required policy passes (right away when none is
        // pending), so wshm never bypasses a branch policy.
        self.rest.require_auth("Azure DevOps complete PR")?;
        let strategy = merge_strategy(strategy)?;
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
        let (me, _) = self
            .me()
            .await
            .context("Azure DevOps: PAT owner unknown, cannot set auto-complete")?;
        self.rest
            .send_json(
                Method::PATCH,
                &self.repo_url(&format!("pullrequests/{number}")),
                &json!({
                    "autoCompleteSetBy": {"id": me},
                    "completionOptions": {
                        "mergeStrategy": strategy,
                        "deleteSourceBranch": false,
                        "transitionWorkItems": false,
                    },
                }),
            )
            .await?;
        info!("Azure DevOps PR {number}: auto-complete set ({strategy})");
        Ok(())
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
        // "Collaborator" = may push to this repository:
        //  1. the identity is resolved by exact account name / mail;
        //  2. an explicit Contribute deny on the repository, for the user or
        //     for one of the project writer groups, wins (denied);
        //  3. an explicit Contribute allow for the user grants;
        //  4. otherwise, membership (direct or nested) of the project's
        //     Contributors or Project Administrators group grants.
        // Anything that cannot be established denies. Other setups (custom
        // groups, org-level rights): list the users in [fix] allowed_users
        // or [ai.injection] trusted_authors.
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
        let mut groups = Vec::new();
        for g in WRITER_GROUPS {
            if let Some(d) = self.group_descriptor(g).await? {
                groups.push(d);
            }
        }
        let (repo_id, project_id) = self.repo_ids().await?;
        let token = format!("repoV2/{project_id}/{repo_id}");
        let all: Vec<&str> = std::iter::once(descriptor.as_str())
            .chain(groups.iter().map(String::as_str))
            .collect();
        let url = self.org_url(
            &format!(
                "accesscontrollists/{GIT_NAMESPACE}?token={}&descriptors={}&includeExtendedInfo=true",
                enc(&token),
                enc(&all.join(","))
            ),
            API,
        );
        let acls = self.rest.get_json(&url).await?;
        let (mut denied, mut user_allowed) = (false, false);
        for acl in acls["value"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let Some(aces) = acl["acesDictionary"].as_object() else {
                continue;
            };
            for (key, ace) in aces {
                let who = ace["descriptor"].as_str().unwrap_or(key);
                if !all.iter().any(|d| d.eq_ignore_ascii_case(who)) {
                    continue;
                }
                let bits = |effective: &str, explicit: &str| {
                    ace["extendedInfo"][effective]
                        .as_u64()
                        .or_else(|| ace[explicit].as_u64())
                        .unwrap_or(0)
                };
                if bits("effectiveDeny", "deny") & GENERIC_CONTRIBUTE != 0 {
                    denied = true;
                }
                if who.eq_ignore_ascii_case(&descriptor)
                    && bits("effectiveAllow", "allow") & GENERIC_CONTRIBUTE != 0
                {
                    user_allowed = true;
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
        if user_allowed {
            return Ok(true);
        }
        let member = user["memberOf"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(descriptor_str)
                    .any(|m| groups.iter().any(|g| g.eq_ignore_ascii_case(&m)))
            })
            .unwrap_or(false);
        if !member {
            info!(
                "Azure DevOps: {username} has no Contribute right on {} and is not in the \
                 project's {WRITER_GROUPS:?} groups — denied",
                self.repo
            );
        }
        Ok(member)
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
    async fn merge_sets_auto_complete_with_strategy() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pullrequests/5"),
            Reply::json(
                json!({"pullRequestId": 5, "status": "active", "mergeStatus": "succeeded"}),
            ),
        );
        server.on(
            "PATCH",
            &format!("{R}/pullrequests/5"),
            Reply::json(json!({"pullRequestId": 5})),
        );
        connection_data(&server);
        let p = provider(&server);
        assert!(p.supports_merge());
        p.merge_pr(5, "squash").await.unwrap();
        let patch = server
            .requests()
            .into_iter()
            .find(|r| r.method == "PATCH")
            .unwrap();
        assert_eq!(
            patch.json(),
            json!({"autoCompleteSetBy": {"id": "me-1"},
                   "completionOptions": {"mergeStrategy": "squash", "deleteSourceBranch": false,
                                         "transitionWorkItems": false}})
        );
        assert_eq!(p.self_username().await.as_deref(), Some("wshm-bot@x.io"));
        assert!(p.merge_pr(5, "octopus").await.is_err());
    }

    #[tokio::test]
    async fn merge_refuses_conflicts_drafts_and_closed_prs() {
        for (pr, why) in [
            (
                json!({"status": "active", "mergeStatus": "conflicts"}),
                "conflicts",
            ),
            (json!({"status": "active", "isDraft": true}), "draft"),
            (json!({"status": "completed"}), "not active"),
        ] {
            let server = MockServer::start().await;
            server.on("GET", &format!("{R}/pullrequests/5"), Reply::json(pr));
            connection_data(&server);
            let err = provider(&server).merge_pr(5, "merge").await.unwrap_err();
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
        assert!(anon.merge_pr(5, "merge").await.is_err());
        assert!(server.requests().is_empty());
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

    /// Identity search answering the user (with `memberOf`) and the
    /// project groups.
    fn identities(server: &MockServer, user: Value) {
        server.on(
            "GET",
            "/acme/_apis/identities",
            Reply::json(json!({"value": [
                {"providerDisplayName": "[Proj]\\Contributors", "descriptor": "g-contrib"}
            ]}))
            .when_query("filterValue=%5BProj%5D%5CContributors"),
        );
        server.on(
            "GET",
            "/acme/_apis/identities",
            Reply::json(json!({"value": [
                {"providerDisplayName": "[Proj]\\Project Administrators", "descriptor": "g-admin"}
            ]}))
            .when_query("Project%20Administrators"),
        );
        server.on(
            "GET",
            "/acme/_apis/identities",
            Reply::json(json!({"value": [user]})),
        );
        server.on(
            "GET",
            R,
            Reply::json(json!({"id": "r1", "project": {"id": "p1"}})),
        );
    }

    fn acl(server: &MockServer, aces: Value) {
        server.on(
            "GET",
            &format!("/acme/_apis/accesscontrollists/{GIT_NAMESPACE}"),
            Reply::json(json!({"value": [{"acesDictionary": aces}]})),
        );
    }

    fn bob(member_of: Value) -> Value {
        json!({"id": "u1", "descriptor": "d-bob", "memberOf": member_of,
               "properties": {"Account": {"$value": "bob@x.io"}}})
    }

    #[tokio::test]
    async fn collaborator_by_project_group_membership() {
        let server = MockServer::start().await;
        identities(&server, bob(json!(["g-other", "g-contrib"])));
        acl(&server, json!({}));
        assert!(provider(&server).is_collaborator("bob@x.io").await.unwrap());
        let reqs = server.requests();
        let user_q = reqs
            .iter()
            .find(|r| r.query.contains("bob%40x.io"))
            .unwrap();
        assert!(user_q.query.contains("queryMembership=ExpandedUp"));
        let acl_req = reqs
            .iter()
            .find(|r| r.path.contains("accesscontrollists"))
            .unwrap();
        assert!(acl_req.query.contains("token=repoV2%2Fp1%2Fr1"));
        assert!(
            acl_req
                .query
                .contains("descriptors=d-bob%2Cg-contrib%2Cg-admin"),
            "{}",
            acl_req.query
        );

        // Matched by mail too; a member of no writer group is denied.
        let server = MockServer::start().await;
        identities(
            &server,
            json!({"descriptor": "d-bob", "memberOf": ["g-readers"],
                   "properties": {"Mail": {"$value": "bob@x.io"}}}),
        );
        acl(&server, json!({}));
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());
        assert!(server
            .requests()
            .iter()
            .any(|r| r.path.contains("accesscontrollists")));
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
    async fn collaborator_explicit_allow_and_deny() {
        // Explicit allow for the user, not in any writer group.
        let server = MockServer::start().await;
        identities(&server, bob(json!([])));
        acl(
            &server,
            json!({"d-bob": {"descriptor": "d-bob", "allow": 6, "deny": 0}}),
        );
        assert!(provider(&server).is_collaborator("bob@x.io").await.unwrap());

        // Allow bit without Contribute (Read only): denied.
        let server = MockServer::start().await;
        identities(&server, bob(json!([])));
        acl(
            &server,
            json!({"d-bob": {"allow": 2, "deny": 0,
                                       "extendedInfo": {"effectiveAllow": 2}}}),
        );
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());

        // Contributors member, but Contribute denied to the group on this repo.
        let server = MockServer::start().await;
        identities(&server, bob(json!(["g-contrib"])));
        acl(&server, json!({"g-contrib": {"allow": 0, "deny": 4}}));
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());

        // Explicit user deny beats membership.
        let server = MockServer::start().await;
        identities(&server, bob(json!(["g-admin"])));
        acl(
            &server,
            json!({"d-bob": {"allow": 0, "deny": 0,
                                       "extendedInfo": {"effectiveDeny": 4}}}),
        );
        assert!(!provider(&server).is_collaborator("bob@x.io").await.unwrap());
    }

    #[tokio::test]
    async fn collaborator_needs_an_exact_identity() {
        // The identity search is fuzzy: "bob" also returns bobby, who is a
        // contributor. Only an exact account/mail match counts.
        let server = MockServer::start().await;
        identities(
            &server,
            json!({"descriptor": "d-bobby", "memberOf": ["g-contrib"],
                   "properties": {"Account": {"$value": "bobby@x.io"}}}),
        );
        acl(&server, json!({}));
        assert!(!provider(&server).is_collaborator("bob").await.unwrap());
        assert!(!server
            .requests()
            .iter()
            .any(|r| r.path.contains("accesscontrollists")));

        let server = MockServer::start().await;
        server.on(
            "GET",
            "/acme/_apis/identities",
            Reply::json(json!({"value": []})),
        );
        assert!(!provider(&server).is_collaborator("ghost").await.unwrap());
        assert!(provider(&server).is_collaborator("a&b").await.is_err());

        // ACL unavailable: an error, never a grant.
        let server = MockServer::start().await;
        identities(&server, bob(json!(["g-contrib"])));
        assert!(provider(&server).is_collaborator("bob@x.io").await.is_err());
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
}
