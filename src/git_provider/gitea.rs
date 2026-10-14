//! Gitea / Forgejo provider — REST API v1 (gitea.com, codeberg.org,
//! self-hosted). Pull requests are issues in Gitea: labels, assignees and
//! comments go through the issue endpoints for both.

use std::collections::HashMap;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::StreamExt;
use reqwest::Method;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::config::Config;
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;
use crate::github::issues::ensure_comment_marker;

use super::http::{self, Auth, Rest};
use super::{Capability, CommentRef, GitProvider, MergedPr, PolledComment, Thread};

/// Items per page. Gitea caps it server-side (MAX_RESPONSE_ITEMS, default
/// 50), so pagination follows the `Link` header rather than counting.
const LIMIT: u32 = 50;
const PER_PR_CONCURRENCY: usize = 6;
const PER_PR_CAP: usize = 200;

pub struct GiteaProvider {
    rest: Rest,
    /// `https://host/api/v1`
    api: String,
    owner: String,
    repo: String,
    marker: String,
    /// "gitea" or "forgejo" (logging only — same API).
    flavor: &'static str,
}

impl GiteaProvider {
    pub fn new(config: &Config, base_url: Option<&str>, flavor: &'static str) -> Result<Self> {
        let default_base = if flavor == "forgejo" {
            "https://codeberg.org"
        } else {
            "https://gitea.com"
        };
        let base = base_url.unwrap_or(default_base);
        let auth = match super::forge_token(config, flavor) {
            Some(t) => Auth::Header("Authorization", format!("token {t}")),
            None => {
                warn!(
                    "{flavor} provider built without a token — anonymous, public repos \
                     read-only. Add a {flavor}_token in Settings → Secrets."
                );
                Auth::None
            }
        };
        let label = if flavor == "forgejo" {
            "Forgejo"
        } else {
            "Gitea"
        };
        let p = Self::with_rest(
            Rest::new(label, auth)?,
            base,
            &config.repo_owner,
            &config.repo_name,
            config.branding.comment_marker(),
            flavor,
        );
        info!(
            "{label} provider initialized: {}/repos/{}/{}",
            p.api, p.owner, p.repo
        );
        Ok(p)
    }

    pub(crate) fn with_rest(
        rest: Rest,
        base: &str,
        owner: &str,
        repo: &str,
        marker: String,
        flavor: &'static str,
    ) -> Self {
        Self {
            rest,
            api: format!("{}/api/v1", base.trim_end_matches('/')),
            owner: owner.to_string(),
            repo: repo.to_string(),
            marker,
            flavor,
        }
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/repos/{}/{}/{}",
            self.api,
            urlencoding::encode(&self.owner),
            urlencoding::encode(&self.repo),
            path
        )
    }

    /// Page through a list endpoint, calling `on_item` until it returns
    /// `false`. The `Link: rel="next"` header only says whether another
    /// page exists: its absolute URL is built from the server ROOT_URL,
    /// which may differ from `git_url` (proxy, internal host), so the next
    /// URL is rebuilt from our base and the page number.
    async fn list(&self, path: &str, mut on_item: impl FnMut(&Value) -> bool) -> Result<()> {
        let sep = if path.contains('?') { '&' } else { '?' };
        for page in 1..=http::MAX_PAGES {
            let url = format!("{}{sep}limit={LIMIT}&page={page}", self.url(path));
            let (body, headers) = self.rest.get_json_with_headers(&url).await?;
            // Gitea answers `null` for some empty lists.
            let empty = Vec::new();
            let items = match &body {
                Value::Null => &empty,
                v => v.as_array().with_context(|| {
                    format!("{}: expected a JSON array from {path}", self.rest.label())
                })?,
            };
            for item in items {
                if !on_item(item) {
                    return Ok(());
                }
            }
            let has_next = headers
                .get("link")
                .and_then(|v| v.to_str().ok())
                .and_then(crate::github::parse_link_next)
                .is_some();
            if !has_next || items.is_empty() {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn list_all(&self, path: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        self.list(path, |v| {
            out.push(v.clone());
            true
        })
        .await?;
        Ok(out)
    }

    async fn open_pulls_raw(&self) -> Result<Vec<Value>> {
        self.list_all("pulls?state=open&sort=recentupdate").await
    }

    /// Label name → id. Gitea's issue-label endpoints take ids; labels a
    /// repo does not have yet are created (GitHub auto-creates them too).
    async fn label_ids(&self, names: &[String], create_missing: bool) -> Result<Vec<u64>> {
        let mut known: HashMap<String, u64> = HashMap::new();
        for l in self.list_all("labels").await? {
            if let (Some(name), Some(id)) = (l["name"].as_str(), l["id"].as_u64()) {
                known.insert(name.to_lowercase(), id);
            }
        }
        // Organization-level labels are usable on org repos.
        let org_url = format!(
            "{}/orgs/{}/labels?limit={LIMIT}",
            self.api,
            urlencoding::encode(&self.owner)
        );
        if let Ok(Some(org)) = self.rest.get_json_opt(&org_url).await {
            for l in org.as_array().map(Vec::as_slice).unwrap_or_default() {
                if let (Some(name), Some(id)) = (l["name"].as_str(), l["id"].as_u64()) {
                    known.entry(name.to_lowercase()).or_insert(id);
                }
            }
        }
        let mut ids = Vec::new();
        for name in names {
            match known.get(&name.to_lowercase()) {
                Some(id) => ids.push(*id),
                None if create_missing => {
                    let created = self
                        .rest
                        .send_json(
                            Method::POST,
                            &self.url("labels"),
                            &json!({ "name": name, "color": "#ededed" }),
                        )
                        .await
                        .with_context(|| format!("create label '{name}'"))?;
                    ids.push(created["id"].as_u64().context("label id missing")?);
                }
                None => {}
            }
        }
        Ok(ids)
    }

    async fn add_labels(&self, number: u64, labels: &[String]) -> Result<()> {
        self.rest.require_auth("label")?;
        if labels.is_empty() {
            return Ok(());
        }
        let ids = self.label_ids(labels, true).await?;
        self.rest
            .send_json(
                Method::POST,
                &self.url(&format!("issues/{number}/labels")),
                &json!({ "labels": ids }),
            )
            .await?;
        Ok(())
    }

    async fn delete_label(&self, number: u64, label: &str) -> Result<()> {
        self.rest.require_auth("remove label")?;
        // An unknown label cannot be on the issue: nothing to remove.
        for id in self.label_ids(&[label.to_string()], false).await? {
            self.rest
                .delete(&self.url(&format!("issues/{number}/labels/{id}")))
                .await?;
        }
        Ok(())
    }

    async fn assign(&self, number: u64, assignees: &[String]) -> Result<()> {
        self.rest.require_auth("assign")?;
        let issue = self
            .rest
            .get_json(&self.url(&format!("issues/{number}")))
            .await?;
        let mut logins: Vec<String> = issue["assignees"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|u| u["login"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for a in assignees {
            if !logins.iter().any(|l| l.eq_ignore_ascii_case(a)) {
                logins.push(a.clone());
            }
        }
        self.rest
            .send_json(
                Method::PATCH,
                &self.url(&format!("issues/{number}")),
                &json!({ "assignees": logins }),
            )
            .await?;
        Ok(())
    }

    async fn upsert_comment(&self, thread: Thread, body: &str) -> Result<()> {
        self.rest.require_auth("comment")?;
        let body = ensure_comment_marker(body, &self.marker);
        match self.find_comment_with_marker(thread, &self.marker).await? {
            Some(existing) => {
                info!(
                    "Updating existing wshm comment {} on #{}",
                    existing.id,
                    thread.number()
                );
                self.update_comment(&existing, &body).await
            }
            None => {
                self.rest
                    .send_json(
                        Method::POST,
                        &self.url(&format!("issues/{}/comments", thread.number())),
                        &json!({ "body": body }),
                    )
                    .await?;
                Ok(())
            }
        }
    }
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

fn parse_issue(i: &Value) -> Option<Issue> {
    if !i["pull_request"].is_null() {
        return None;
    }
    Some(Issue {
        number: i["number"].as_u64().unwrap_or(0),
        title: i["title"].as_str().unwrap_or("").to_string(),
        body: s(&i["body"]),
        state: i["state"].as_str().unwrap_or("open").to_string(),
        labels: http::label_names(&i["labels"]),
        author: s(&i["user"]["login"]),
        created_at: i["created_at"].as_str().unwrap_or("").to_string(),
        updated_at: i["updated_at"].as_str().unwrap_or("").to_string(),
        reactions_plus1: 0,
        reactions_total: 0,
    })
}

fn parse_pull(pr: &Value) -> PullRequest {
    PullRequest {
        number: pr["number"].as_u64().unwrap_or(0),
        title: pr["title"].as_str().unwrap_or("").to_string(),
        body: s(&pr["body"]),
        state: pr["state"].as_str().unwrap_or("open").to_string(),
        labels: http::label_names(&pr["labels"]),
        author: s(&pr["user"]["login"]),
        head_sha: s(&pr["head"]["sha"]),
        base_sha: s(&pr["base"]["sha"]),
        head_ref: s(&pr["head"]["ref"]),
        base_ref: s(&pr["base"]["ref"]),
        mergeable: pr["mergeable"].as_bool(),
        ci_status: None,
        created_at: pr["created_at"].as_str().unwrap_or("").to_string(),
        updated_at: pr["updated_at"].as_str().unwrap_or("").to_string(),
        review_decision: None,
        review_decision_at: None,
        reactions_plus1: 0,
    }
}

/// Issue number from a comment's `issue_url` (`…/issues/12` or `…/pulls/12`).
fn number_from_url(url: &str) -> Option<u64> {
    url.trim_end_matches('/').rsplit('/').next()?.parse().ok()
}

#[async_trait]
impl GitProvider for GiteaProvider {
    fn provider_name(&self) -> &str {
        self.flavor
    }

    fn repo_slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    fn comment_marker(&self) -> &str {
        &self.marker
    }

    fn is_authenticated(&self) -> bool {
        self.rest.is_authenticated()
    }

    async fn self_username(&self) -> Option<String> {
        self.rest
            .self_login(&format!("{}/user", self.api), "login")
            .await
    }

    fn supports(&self, cap: Capability) -> bool {
        match cap {
            // Gitea/Forgejo render ```mermaid client-side wherever Markdown
            // is rendered (files, issues, PRs, comments); see the
            // MERMAID_MAX_SOURCE_CHARACTERS [markup] setting.
            Capability::Mermaid => true,
            Capability::ReviewDecisions
            | Capability::Reactions
            | Capability::CiStatus
            | Capability::CommentPolling => true,
        }
    }

    async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        let mut path = "issues?state=open&type=issues".to_string();
        if let Some(s) = since {
            path.push_str(&format!("&since={}", urlencoding::encode(s)));
        }
        Ok(self
            .list_all(&path)
            .await?
            .iter()
            .filter_map(parse_issue)
            .collect())
    }

    async fn fetch_all_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        let mut path = "issues?state=all&type=issues".to_string();
        if let Some(s) = since {
            path.push_str(&format!("&since={}", urlencoding::encode(s)));
        }
        Ok(self
            .list_all(&path)
            .await?
            .iter()
            .filter_map(parse_issue)
            .collect())
    }

    async fn fetch_issue(&self, number: u64) -> Result<Option<Issue>> {
        Ok(self
            .rest
            .get_json_opt(&self.url(&format!("issues/{number}")))
            .await?
            .and_then(|v| parse_issue(&v)))
    }

    async fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        self.rest.require_auth("create issue")?;
        let ids = if labels.is_empty() {
            Vec::new()
        } else {
            self.label_ids(labels, true).await?
        };
        let resp = self
            .rest
            .send_json(
                Method::POST,
                &self.url("issues"),
                &json!({
                    "title": title,
                    "body": ensure_comment_marker(body, &self.marker),
                    "labels": ids,
                }),
            )
            .await?;
        resp["number"]
            .as_u64()
            .context("missing issue number in response")
    }

    async fn close_issue(&self, number: u64) -> Result<()> {
        self.rest.require_auth("close issue")?;
        self.rest
            .send_json(
                Method::PATCH,
                &self.url(&format!("issues/{number}")),
                &json!({ "state": "closed" }),
            )
            .await?;
        Ok(())
    }

    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        self.add_labels(number, labels).await
    }

    async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        self.delete_label(number, label).await
    }

    async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        self.assign(number, assignees).await
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
        let mut out = Vec::new();
        self.list(&format!("pulls?state={state}&sort=recentupdate"), |pr| {
            if let Some(s) = since {
                if http::ts_before(pr["updated_at"].as_str().unwrap_or(""), s) {
                    return false;
                }
            }
            out.push(parse_pull(pr));
            true
        })
        .await?;
        Ok(out)
    }

    async fn fetch_pull(&self, number: u64) -> Result<Option<PullRequest>> {
        Ok(self
            .rest
            .get_json_opt(&self.url(&format!("pulls/{number}")))
            .await?
            .map(|v| parse_pull(&v)))
    }

    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>> {
        let pr = self
            .rest
            .get_json(&self.url(&format!("pulls/{number}")))
            .await?;
        Ok(pr["mergeable"].as_bool())
    }

    async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        self.rest
            .get_text(
                &self.url(&format!("pulls/{number}.diff")),
                Some("text/plain"),
            )
            .await
    }

    async fn fetch_merged_pulls(&self, since: Option<&str>) -> Result<Vec<MergedPr>> {
        let mut out = Vec::new();
        self.list("pulls?state=closed&sort=recentupdate", |pr| {
            let Some(merged_at) = pr["merged_at"].as_str() else {
                return true;
            };
            if let Some(s) = since {
                if http::ts_before(merged_at, s) {
                    return false;
                }
            }
            out.push(MergedPr {
                number: pr["number"].as_u64().unwrap_or(0),
                title: pr["title"].as_str().unwrap_or("").to_string(),
                body: s(&pr["body"]),
                author: s(&pr["user"]["login"]),
                labels: http::label_names(&pr["labels"]),
                merged_at: merged_at.to_string(),
                created_at: pr["created_at"].as_str().unwrap_or("").to_string(),
            });
            true
        })
        .await?;
        Ok(out)
    }

    async fn fetch_review_decisions(&self) -> Result<HashMap<u64, Option<String>>> {
        let pulls = self.open_pulls_raw().await?;
        let targets: Vec<(u64, bool)> = pulls
            .iter()
            .take(PER_PR_CAP)
            .filter_map(|pr| {
                let requested = pr["requested_reviewers"]
                    .as_array()
                    .is_some_and(|r| !r.is_empty());
                Some((pr["number"].as_u64()?, requested))
            })
            .collect();
        let jobs = targets.into_iter().map(|(n, requested)| async move {
            (
                n,
                requested,
                self.list_all(&format!("pulls/{n}/reviews")).await,
            )
        });
        let results: Vec<_> = futures::stream::iter(jobs)
            .buffer_unordered(PER_PR_CONCURRENCY)
            .collect()
            .await;
        let mut map = HashMap::new();
        for (n, requested, reviews) in results {
            // Latest non-comment verdict per reviewer.
            let mut latest: HashMap<String, String> = HashMap::new();
            for r in reviews? {
                if r["dismissed"].as_bool() == Some(true) || r["stale"].as_bool() == Some(true) {
                    continue;
                }
                let state = r["state"].as_str().unwrap_or("");
                if matches!(state, "APPROVED" | "REQUEST_CHANGES") {
                    let who = r["user"]["login"].as_str().unwrap_or("").to_string();
                    latest.insert(who, state.to_string());
                }
            }
            let decision = if latest.values().any(|s| s == "REQUEST_CHANGES") {
                Some("changes_requested")
            } else if latest.values().any(|s| s == "APPROVED") {
                Some("approved")
            } else if requested {
                Some("review_required")
            } else {
                None
            };
            if let Some(d) = decision {
                map.insert(n, Some(d.to_string()));
            }
        }
        Ok(map)
    }

    async fn fetch_pull_reactions(&self) -> Result<HashMap<u64, u32>> {
        let pulls = self.open_pulls_raw().await?;
        let targets: Vec<u64> = pulls
            .iter()
            .take(PER_PR_CAP)
            .filter_map(|pr| pr["number"].as_u64())
            .collect();
        let jobs = targets.into_iter().map(|n| async move {
            let url = self.url(&format!("issues/{n}/reactions"));
            (n, self.rest.get_json(&url).await)
        });
        let results: Vec<_> = futures::stream::iter(jobs)
            .buffer_unordered(PER_PR_CONCURRENCY)
            .collect()
            .await;
        let mut map = HashMap::new();
        for (n, res) in results {
            let plus1 = res?
                .as_array()
                .map(|a| a.iter().filter(|r| r["content"] == "+1").count())
                .unwrap_or(0);
            map.insert(n, plus1 as u32);
        }
        Ok(map)
    }

    async fn fetch_ci_statuses(&self) -> Result<HashMap<u64, Option<String>>> {
        let pulls = self.open_pulls_raw().await?;
        let targets: Vec<(u64, String)> = pulls
            .iter()
            .take(PER_PR_CAP)
            .filter_map(|pr| {
                Some((
                    pr["number"].as_u64()?,
                    pr["head"]["sha"].as_str()?.to_string(),
                ))
            })
            .collect();
        let jobs = targets.into_iter().map(|(n, sha)| async move {
            let url = self.url(&format!("commits/{sha}/status"));
            (n, self.rest.get_json(&url).await)
        });
        let results: Vec<_> = futures::stream::iter(jobs)
            .buffer_unordered(PER_PR_CONCURRENCY)
            .collect()
            .await;
        let mut map = HashMap::new();
        for (n, res) in results {
            let status = res?;
            if status["total_count"].as_u64() == Some(0) {
                continue;
            }
            if let Some(bucket) = status["state"].as_str().and_then(http::ci_bucket) {
                map.insert(n, Some(bucket.to_string()));
            }
        }
        Ok(map)
    }

    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        self.add_labels(number, labels).await
    }

    async fn remove_pr_label(&self, number: u64, label: &str) -> Result<()> {
        self.delete_label(number, label).await
    }

    async fn add_pr_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        self.assign(number, assignees).await
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
        self.rest.require_auth("submit review")?;
        let comments: Vec<Value> = comments
            .iter()
            .map(|(path, line, b)| json!({ "path": path, "body": ensure_comment_marker(b, &self.marker), "new_position": line }))
            .collect();
        self.rest
            .send_json(
                Method::POST,
                &self.url(&format!("pulls/{pr_number}/reviews")),
                &json!({ "body": ensure_comment_marker(body, &self.marker), "event": "COMMENT", "comments": comments }),
            )
            .await?;
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
        self.rest.require_auth("create PR")?;
        // Gitea/Forgejo mark work-in-progress PRs by title prefix
        // ([repository.pull-request] WORK_IN_PROGRESS_PREFIXES, default "WIP:").
        let title = if draft {
            format!("WIP: {title}")
        } else {
            title.to_string()
        };
        let resp = self
            .rest
            .send_json(
                Method::POST,
                &self.url("pulls"),
                &json!({ "title": title, "body": body, "head": head, "base": base }),
            )
            .await?;
        resp["number"]
            .as_u64()
            .context("missing PR number in response")
    }

    async fn find_comment_with_marker(
        &self,
        thread: Thread,
        marker: &str,
    ) -> Result<Option<CommentRef>> {
        let mut found = None;
        self.list(&format!("issues/{}/comments", thread.number()), |c| {
            if http::has_marker(c["body"].as_str().unwrap_or(""), marker) {
                found = c["id"].as_u64();
            }
            found.is_none()
        })
        .await?;
        Ok(found.map(|id| CommentRef {
            thread,
            id,
            parent_id: None,
        }))
    }

    async fn update_comment(&self, comment: &CommentRef, body: &str) -> Result<()> {
        self.rest.require_auth("update comment")?;
        self.rest
            .send_json(
                Method::PATCH,
                &self.url(&format!("issues/comments/{}", comment.id)),
                &json!({ "body": body }),
            )
            .await?;
        Ok(())
    }

    async fn delete_comment(&self, comment: &CommentRef) -> Result<()> {
        self.rest.require_auth("delete comment")?;
        self.rest
            .delete(&self.url(&format!("issues/comments/{}", comment.id)))
            .await
    }

    async fn fetch_comments_since(&self, since: &str) -> Result<Vec<PolledComment>> {
        let mut out = Vec::new();
        self.list(
            &format!("issues/comments?since={}", urlencoding::encode(since)),
            |c| {
                let created = c["created_at"].as_str().unwrap_or("");
                if !http::ts_before(since, created) {
                    return true;
                }
                let pr_url = c["pull_request_url"].as_str().unwrap_or("");
                let issue_url = c["issue_url"].as_str().unwrap_or("");
                let number = number_from_url(if pr_url.is_empty() { issue_url } else { pr_url });
                if let (Some(id), Some(number)) = (c["id"].as_u64(), number) {
                    out.push(PolledComment {
                        id,
                        number,
                        is_pr: !pr_url.is_empty(),
                        author: c["user"]["login"].as_str().unwrap_or("").to_string(),
                        body: c["body"].as_str().unwrap_or("").to_string(),
                        created_at: created.to_string(),
                    });
                }
                true
            },
        )
        .await?;
        Ok(out)
    }

    async fn is_collaborator(&self, username: &str) -> Result<bool> {
        if username.is_empty()
            || !username
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            anyhow::bail!("Invalid {} username: {username}", self.flavor);
        }
        let url = self.url(&format!(
            "collaborators/{}/permission",
            urlencoding::encode(username)
        ));
        // 404/403 → not a collaborator (or not visible to the token).
        let resp = self.rest.send(Method::GET, &url, None, None, None).await?;
        if resp.status.as_u16() == 404 {
            return Ok(false);
        }
        if resp.status.as_u16() == 403 {
            warn!(
                "{}: collaborator check for {username} refused (403) — the API token needs \
                 admin rights on {}/{} to read other users' permissions; denying",
                self.flavor, self.owner, self.repo
            );
            return Ok(false);
        }
        if !resp.status.is_success() {
            anyhow::bail!(
                "{} collaborator check for {username} failed: HTTP {}",
                self.flavor,
                resp.status
            );
        }
        let v: Value = serde_json::from_str(&resp.text).unwrap_or_default();
        Ok(matches!(
            v["permission"].as_str().unwrap_or("none"),
            "admin" | "write" | "owner"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_provider::mock::{MockServer, Reply};

    const R: &str = "/api/v1/repos/acme/widgets";

    fn provider(server: &MockServer) -> GiteaProvider {
        GiteaProvider::with_rest(
            Rest::new("Gitea", Auth::Header("Authorization", "token tok".into())).unwrap(),
            &server.url(),
            "acme",
            "widgets",
            "<!-- wshm -->".into(),
            "gitea",
        )
    }

    #[tokio::test]
    async fn issues_follow_link_pagination_and_skip_pulls() {
        let server = MockServer::start().await;
        let next = format!(
            // ROOT_URL differs from git_url: must not be followed.
            "<https://public.example{}/issues?state=all&type=issues&limit=50&page=2>; rel=\"next\"",
            R
        );
        server.on(
            "GET",
            &format!("{R}/issues"),
            Reply::json(json!([
                {"number": 1, "title": "a", "state": "open", "labels": [{"name": "bug"}],
                 "user": {"login": "alice"}, "pull_request": null},
                {"number": 2, "title": "pr", "state": "open", "pull_request": {"merged": false}}
            ]))
            .header("link", &next)
            .when_query("page=1"),
        );
        server.on(
            "GET",
            &format!("{R}/issues"),
            Reply::json(json!([{"number": 3, "title": "c", "state": "closed"}]))
                .when_query("page=2"),
        );
        let issues = provider(&server).fetch_all_issues(None).await.unwrap();
        let numbers: Vec<u64> = issues.iter().map(|i| i.number).collect();
        assert_eq!(numbers, vec![1, 3]);
        assert_eq!(issues[0].labels, vec!["bug"]);
        assert_eq!(
            server.requests()[0].header("authorization").as_deref(),
            Some("token tok")
        );
    }

    #[tokio::test]
    async fn labels_resolve_ids_and_create_missing() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/labels"),
            Reply::json(json!([{"id": 7, "name": "Bug"}])),
        );
        server.on(
            "GET",
            "/api/v1/orgs/acme/labels",
            Reply::json(json!([])).status(404),
        );
        server.on(
            "POST",
            &format!("{R}/labels"),
            Reply::json(json!({"id": 9, "name": "new"})),
        );
        server.on(
            "POST",
            &format!("{R}/issues/5/labels"),
            Reply::json(json!([])),
        );
        provider(&server)
            .label_pr(5, &["bug".into(), "new".into()])
            .await
            .unwrap();
        let reqs = server.requests();
        let add = reqs
            .iter()
            .find(|r| r.path == format!("{R}/issues/5/labels"))
            .unwrap();
        assert_eq!(add.json(), json!({"labels": [7, 9]}));
    }

    #[tokio::test]
    async fn remove_unknown_label_is_a_noop_request_wise() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/labels"),
            Reply::json(json!([{"id": 7, "name": "bug"}])),
        );
        server.on(
            "DELETE",
            &format!("{R}/issues/5/labels/7"),
            Reply::json(json!({})).status(204),
        );
        let p = provider(&server);
        p.remove_label(5, "missing").await.unwrap();
        assert!(!server.requests().iter().any(|r| r.method == "DELETE"));
        p.remove_label(5, "BUG").await.unwrap();
        assert!(server.requests().iter().any(|r| r.method == "DELETE"));
    }

    #[tokio::test]
    async fn comment_upsert_and_review_payload() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/issues/3/comments"),
            Reply::json(json!([{"id": 1, "body": "x"}, {"id": 2, "body": "y <!-- wshm -->"}])),
        );
        server.on(
            "PATCH",
            &format!("{R}/issues/comments/2"),
            Reply::json(json!({})),
        );
        server.on(
            "POST",
            &format!("{R}/pulls/3/reviews"),
            Reply::json(json!({})),
        );
        let p = provider(&server);
        p.comment_pr(3, "hello").await.unwrap();
        p.submit_review(3, "sum", &[("a.rs".into(), 4, "nit".into())])
            .await
            .unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[1].json(), json!({"body": "hello\n\n<!-- wshm -->"}));
        assert_eq!(
            reqs[2].json(),
            json!({"body": "sum\n\n<!-- wshm -->", "event": "COMMENT",
                   "comments": [{"path": "a.rs", "body": "nit\n\n<!-- wshm -->", "new_position": 4}]})
        );
    }

    #[tokio::test]
    async fn collaborator_permission_levels() {
        for (status, perm, expected) in [
            (200, "write", true),
            (200, "admin", true),
            (200, "owner", true),
            (200, "read", false),
            (404, "", false),
            (403, "", false),
        ] {
            let server = MockServer::start().await;
            server.on(
                "GET",
                &format!("{R}/collaborators/bob/permission"),
                Reply::json(json!({"permission": perm})).status(status),
            );
            assert_eq!(
                provider(&server).is_collaborator("bob").await.unwrap(),
                expected,
                "{status} {perm}"
            );
        }
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/collaborators/bob/permission"),
            Reply::text("boom").status(500),
        );
        // 5xx is retried then surfaces as an error, never as "allowed".
        assert!(provider(&server).is_collaborator("bob").await.is_err());
    }

    #[tokio::test]
    async fn diff_reviews_and_ci() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/pulls/4.diff"),
            Reply::text("diff --git a/x b/x\n"),
        );
        server.on(
            "GET",
            &format!("{R}/pulls"),
            Reply::json(json!([
                {"number": 4, "head": {"sha": "s4"}, "requested_reviewers": []},
                {"number": 5, "head": {"sha": "s5"}, "requested_reviewers": [{"login": "r"}]}
            ])),
        );
        server.on(
            "GET",
            &format!("{R}/pulls/4/reviews"),
            Reply::json(json!([
                {"state": "REQUEST_CHANGES", "user": {"login": "a"}},
                {"state": "APPROVED", "user": {"login": "a"}}
            ])),
        );
        server.on(
            "GET",
            &format!("{R}/pulls/5/reviews"),
            Reply::json(json!([])),
        );
        server.on(
            "GET",
            &format!("{R}/commits/s4/status"),
            Reply::json(json!({"state": "failure", "total_count": 1})),
        );
        server.on(
            "GET",
            &format!("{R}/commits/s5/status"),
            Reply::json(json!({"state": "", "total_count": 0})),
        );
        let p = provider(&server);
        assert_eq!(p.fetch_pr_diff(4).await.unwrap(), "diff --git a/x b/x\n");
        let d = p.fetch_review_decisions().await.unwrap();
        assert_eq!(d[&4].as_deref(), Some("approved"));
        assert_eq!(d[&5].as_deref(), Some("review_required"));
        let ci = p.fetch_ci_statuses().await.unwrap();
        assert_eq!(ci[&4].as_deref(), Some("failure"));
        assert!(!ci.contains_key(&5));
    }

    #[tokio::test]
    async fn polls_repo_wide_comments() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{R}/issues/comments"),
            Reply::json(json!([
                {"id": 1, "body": "/wshm triage", "user": {"login": "bob"},
                 "created_at": "2026-05-01T12:01:00+02:00",
                 "issue_url": "https://h/acme/widgets/issues/12", "pull_request_url": ""},
                {"id": 2, "body": "/wshm analyze", "user": {"login": "eve"},
                 "created_at": "2026-05-01T10:02:00Z",
                 "issue_url": "https://h/acme/widgets/issues/13",
                 "pull_request_url": "https://h/acme/widgets/pulls/13"}
            ])),
        );
        let c = provider(&server)
            .fetch_comments_since("2026-05-01T10:00:00Z")
            .await
            .unwrap();
        // 12:01+02:00 == 10:01Z, after the cursor.
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].number, c[0].is_pr), (12, false));
        assert_eq!(
            (c[1].number, c[1].is_pr, c[1].author.as_str()),
            (13, true, "eve")
        );
    }
}
