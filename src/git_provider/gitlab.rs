//! GitLab provider — REST API v4 (GitLab.com and self-managed).
//!
//! The project is addressed by its URL-encoded full path (`group/sub/repo`),
//! issues and merge requests by their project-scoped `iid`.

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

/// Concurrency for per-MR enrichment calls (approvals).
const PER_MR_CONCURRENCY: usize = 6;
/// Cap on MRs enriched per sync pass, to bound API usage.
const PER_MR_CAP: usize = 200;

pub struct GitLabProvider {
    rest: Rest,
    /// `https://gitlab.example.com/api/v4`
    api: String,
    /// URL-encoded project path.
    project: String,
    slug: String,
    marker: String,
}

impl GitLabProvider {
    pub fn new(config: &Config, base_url: Option<&str>) -> Result<Self> {
        let base = base_url.unwrap_or("https://gitlab.com");
        let auth = match super::forge_token(config, "gitlab") {
            Some(t) => Auth::Header("Authorization", format!("Bearer {t}")),
            None => {
                warn!(
                    "GitLab provider built without a token — anonymous, public projects \
                     read-only. Add a gitlab_token in Settings → Secrets or set GITLAB_TOKEN."
                );
                Auth::None
            }
        };
        let p = Self::with_rest(
            Rest::new("GitLab", auth)?,
            base,
            &config.repo_slug(),
            config.branding.comment_marker(),
        );
        info!(
            "GitLab provider initialized: {}/projects/{}",
            p.api, p.project
        );
        Ok(p)
    }

    pub(crate) fn with_rest(rest: Rest, base: &str, slug: &str, marker: String) -> Self {
        Self {
            rest,
            api: format!("{}/api/v4", base.trim_end_matches('/')),
            project: urlencoding::encode(slug).into_owned(),
            slug: slug.to_string(),
            marker,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/projects/{}/{}", self.api, self.project, path)
    }

    fn kind(thread: Thread) -> (&'static str, u64) {
        match thread {
            Thread::Issue(n) => ("issues", n),
            Thread::Pull(n) => ("merge_requests", n),
        }
    }

    /// Follow GitLab offset pagination (`x-next-page`) calling `on_item`
    /// until it returns `false` or the pages run out.
    async fn list(&self, path: &str, mut on_item: impl FnMut(&Value) -> bool) -> Result<()> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut page = 1u64;
        loop {
            let url = format!("{}{sep}per_page=100&page={page}", self.url(path));
            let (body, headers) = self.rest.get_json_with_headers(&url).await?;
            let items = body
                .as_array()
                .with_context(|| format!("GitLab: expected a JSON array from {path}"))?;
            for item in items {
                if !on_item(item) {
                    return Ok(());
                }
            }
            match http::header_u64(&headers, "x-next-page") {
                Some(next) if next > page && page < u64::from(http::MAX_PAGES) => page = next,
                _ => return Ok(()),
            }
        }
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

    async fn open_mrs(&self) -> Result<Vec<Value>> {
        self.list_all("merge_requests?state=opened").await
    }

    /// Resolve a username to its numeric user id.
    async fn user_id(&self, username: &str) -> Result<Option<u64>> {
        validate_username(username)?;
        let url = format!(
            "{}/users?username={}",
            self.api,
            urlencoding::encode(username)
        );
        let users = self.rest.get_json(&url).await?;
        Ok(users
            .as_array()
            .and_then(|a| a.first())
            .and_then(|u| u["id"].as_u64()))
    }

    async fn edit(&self, kind: &str, number: u64, body: Value) -> Result<()> {
        self.rest.require_auth("GitLab edit")?;
        self.rest
            .send_json(Method::PUT, &self.url(&format!("{kind}/{number}")), &body)
            .await?;
        Ok(())
    }

    async fn add_assignees_to(&self, kind: &str, number: u64, assignees: &[String]) -> Result<()> {
        self.rest.require_auth("GitLab assign")?;
        let current = self
            .rest
            .get_json(&self.url(&format!("{kind}/{number}")))
            .await?;
        let mut ids: Vec<u64> = current["assignees"]
            .as_array()
            .map(|a| a.iter().filter_map(|u| u["id"].as_u64()).collect())
            .unwrap_or_default();
        for name in assignees {
            let id = self
                .user_id(name)
                .await?
                .with_context(|| format!("GitLab user '{name}' not found"))?;
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        self.edit(kind, number, json!({ "assignee_ids": ids }))
            .await
    }

    async fn upsert_comment(&self, thread: Thread, body: &str) -> Result<()> {
        self.rest.require_auth("GitLab comment")?;
        let body = ensure_comment_marker(body, &self.marker);
        let (kind, number) = Self::kind(thread);
        match self.find_comment_with_marker(thread, &self.marker).await? {
            Some(existing) => {
                info!(
                    "Updating existing wshm note {} on {kind} !{number}",
                    existing.id
                );
                self.update_comment(&existing, &body).await
            }
            None => {
                self.rest
                    .send_json(
                        Method::POST,
                        &self.url(&format!("{kind}/{number}/notes")),
                        &json!({ "body": body }),
                    )
                    .await?;
                Ok(())
            }
        }
    }
}

fn validate_username(username: &str) -> Result<()> {
    if username.is_empty()
        || !username
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        anyhow::bail!("Invalid GitLab username: {username}");
    }
    Ok(())
}

fn normalize_state(state: &str) -> &'static str {
    match state {
        "opened" => "open",
        _ => "closed", // closed, merged, locked
    }
}

fn parse_issue(i: &Value) -> Issue {
    let up = i["upvotes"].as_u64().unwrap_or(0) as u32;
    let down = i["downvotes"].as_u64().unwrap_or(0) as u32;
    Issue {
        number: i["iid"].as_u64().unwrap_or(0),
        title: i["title"].as_str().unwrap_or("").to_string(),
        body: i["description"].as_str().map(String::from),
        state: normalize_state(i["state"].as_str().unwrap_or("opened")).to_string(),
        labels: http::label_names(&i["labels"]),
        author: i["author"]["username"].as_str().map(String::from),
        created_at: i["created_at"].as_str().unwrap_or("").to_string(),
        updated_at: i["updated_at"].as_str().unwrap_or("").to_string(),
        reactions_plus1: up,
        reactions_total: up + down,
    }
}

/// GitHub's `mergeable` means "no conflicts": map GitLab's `has_conflicts`,
/// leaving it unknown while GitLab is still computing the merge status.
fn mergeable(mr: &Value) -> Option<bool> {
    let status = mr["detailed_merge_status"]
        .as_str()
        .or_else(|| mr["merge_status"].as_str())
        .unwrap_or("");
    if matches!(
        status,
        "unchecked" | "checking" | "preparing" | "cannot_be_merged_recheck"
    ) {
        return None;
    }
    mr["has_conflicts"].as_bool().map(|c| !c)
}

fn parse_mr(mr: &Value) -> PullRequest {
    let s = |v: &Value| v.as_str().map(String::from);
    PullRequest {
        number: mr["iid"].as_u64().unwrap_or(0),
        title: mr["title"].as_str().unwrap_or("").to_string(),
        body: s(&mr["description"]),
        state: normalize_state(mr["state"].as_str().unwrap_or("opened")).to_string(),
        labels: http::label_names(&mr["labels"]),
        author: s(&mr["author"]["username"]),
        head_sha: s(&mr["sha"]).or_else(|| s(&mr["diff_refs"]["head_sha"])),
        base_sha: s(&mr["diff_refs"]["base_sha"]),
        head_ref: s(&mr["source_branch"]),
        base_ref: s(&mr["target_branch"]),
        mergeable: mergeable(mr),
        ci_status: mr["head_pipeline"]["status"]
            .as_str()
            .and_then(http::ci_bucket)
            .map(String::from),
        created_at: mr["created_at"].as_str().unwrap_or("").to_string(),
        updated_at: mr["updated_at"].as_str().unwrap_or("").to_string(),
        review_decision: None,
        review_decision_at: None,
        reactions_plus1: mr["upvotes"].as_u64().unwrap_or(0) as u32,
    }
}

/// Rebuild a `git diff`-style text from GitLab's per-file diff entries.
fn render_diffs(entries: &[Value]) -> String {
    let mut out = String::new();
    for c in entries {
        let old = c["old_path"].as_str().unwrap_or("");
        let new = c["new_path"].as_str().unwrap_or(old);
        out.push_str(&format!("diff --git a/{old} b/{new}\n"));
        let (from, to) = if c["new_file"].as_bool() == Some(true) {
            out.push_str("new file mode 100644\n");
            ("/dev/null".to_string(), format!("b/{new}"))
        } else if c["deleted_file"].as_bool() == Some(true) {
            out.push_str("deleted file mode 100644\n");
            (format!("a/{old}"), "/dev/null".to_string())
        } else {
            if c["renamed_file"].as_bool() == Some(true) {
                out.push_str(&format!("rename from {old}\nrename to {new}\n"));
            }
            (format!("a/{old}"), format!("b/{new}"))
        };
        let diff = c["diff"].as_str().unwrap_or("");
        if diff.is_empty() {
            continue;
        }
        out.push_str(&format!("--- {from}\n+++ {to}\n{diff}"));
        if !diff.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[async_trait]
impl GitProvider for GitLabProvider {
    fn provider_name(&self) -> &str {
        "gitlab"
    }

    fn repo_slug(&self) -> String {
        self.slug.clone()
    }

    fn comment_marker(&self) -> &str {
        &self.marker
    }

    async fn self_username(&self) -> Option<String> {
        self.rest
            .self_login(&format!("{}/user", self.api), "username")
            .await
    }
    fn is_authenticated(&self) -> bool {
        self.rest.is_authenticated()
    }

    fn supports(&self, cap: Capability) -> bool {
        match cap {
            // GLFM renders ```mermaid everywhere Markdown is rendered,
            // comments included: https://docs.gitlab.com/user/markdown/#mermaid
            Capability::Mermaid => true,
            Capability::ReviewDecisions
            | Capability::Reactions
            | Capability::CiStatus
            | Capability::CommentPolling => true,
        }
    }

    async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        let mut path = "issues?state=opened&order_by=updated_at&sort=desc".to_string();
        if let Some(s) = since {
            path.push_str(&format!("&updated_after={}", urlencoding::encode(s)));
        }
        Ok(self
            .list_all(&path)
            .await?
            .iter()
            .map(parse_issue)
            .collect())
    }

    async fn fetch_all_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        let mut path = "issues?state=all&order_by=updated_at&sort=desc".to_string();
        if let Some(s) = since {
            path.push_str(&format!("&updated_after={}", urlencoding::encode(s)));
        }
        Ok(self
            .list_all(&path)
            .await?
            .iter()
            .map(parse_issue)
            .collect())
    }

    async fn fetch_issue(&self, number: u64) -> Result<Option<Issue>> {
        Ok(self
            .rest
            .get_json_opt(&self.url(&format!("issues/{number}")))
            .await?
            .map(|v| parse_issue(&v)))
    }

    async fn create_issue(&self, title: &str, body: &str, labels: &[String]) -> Result<u64> {
        self.rest.require_auth("GitLab create issue")?;
        let resp = self
            .rest
            .send_json(
                Method::POST,
                &self.url("issues"),
                &json!({
                    "title": title,
                    "description": ensure_comment_marker(body, &self.marker),
                    "labels": labels.join(","),
                }),
            )
            .await?;
        resp["iid"]
            .as_u64()
            .context("GitLab: missing iid in create issue response")
    }

    async fn close_issue(&self, number: u64) -> Result<()> {
        self.edit("issues", number, json!({ "state_event": "close" }))
            .await
    }

    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        self.edit("issues", number, json!({ "add_labels": labels.join(",") }))
            .await
    }

    async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        self.edit("issues", number, json!({ "remove_labels": label }))
            .await
    }

    async fn add_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        self.add_assignees_to("issues", number, assignees).await
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
        let gl_state = match state {
            "open" => "opened",
            _ => "all", // GitHub "closed" includes merged; filtered below
        };
        let mut path = format!("merge_requests?state={gl_state}&order_by=updated_at&sort=desc");
        if let Some(s) = since {
            path.push_str(&format!("&updated_after={}", urlencoding::encode(s)));
        }
        let mut out = Vec::new();
        self.list(&path, |mr| {
            let updated = mr["updated_at"].as_str().unwrap_or("");
            if let Some(s) = since {
                if http::ts_before(updated, s) {
                    return false;
                }
            }
            let pr = parse_mr(mr);
            if state != "closed" || pr.state == "closed" {
                out.push(pr);
            }
            true
        })
        .await?;
        Ok(out)
    }

    async fn fetch_pull(&self, number: u64) -> Result<Option<PullRequest>> {
        Ok(self
            .rest
            .get_json_opt(&self.url(&format!("merge_requests/{number}")))
            .await?
            .map(|v| parse_mr(&v)))
    }

    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>> {
        let mr = self
            .rest
            .get_json(&self.url(&format!("merge_requests/{number}")))
            .await?;
        Ok(mergeable(&mr))
    }

    async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        // GET /merge_requests/:iid/diffs (paginated, GitLab 15.7+) — the
        // older /changes endpoint is deprecated.
        let entries = self
            .list_all(&format!("merge_requests/{number}/diffs"))
            .await?;
        Ok(render_diffs(&entries))
    }

    async fn fetch_merged_pulls(&self, since: Option<&str>) -> Result<Vec<MergedPr>> {
        let mut path = "merge_requests?state=merged&order_by=updated_at&sort=desc".to_string();
        if let Some(s) = since {
            path.push_str(&format!("&updated_after={}", urlencoding::encode(s)));
        }
        let mut out = Vec::new();
        self.list(&path, |mr| {
            let Some(merged_at) = mr["merged_at"].as_str() else {
                return true;
            };
            if let Some(s) = since {
                if http::ts_before(merged_at, s) {
                    return false;
                }
            }
            out.push(MergedPr {
                number: mr["iid"].as_u64().unwrap_or(0),
                title: mr["title"].as_str().unwrap_or("").to_string(),
                body: mr["description"].as_str().map(String::from),
                author: mr["author"]["username"].as_str().map(String::from),
                labels: http::label_names(&mr["labels"]),
                merged_at: merged_at.to_string(),
                created_at: mr["created_at"].as_str().unwrap_or("").to_string(),
            });
            true
        })
        .await?;
        Ok(out)
    }

    async fn fetch_review_decisions(&self) -> Result<HashMap<u64, Option<String>>> {
        let mrs = self.open_mrs().await?;
        let targets: Vec<(u64, bool)> = mrs
            .iter()
            .take(PER_MR_CAP)
            .filter_map(|mr| {
                let has_reviewers = mr["reviewers"].as_array().is_some_and(|r| !r.is_empty());
                Some((mr["iid"].as_u64()?, has_reviewers))
            })
            .collect();
        let jobs = targets.into_iter().map(|(iid, has_reviewers)| async move {
            let url = self.url(&format!("merge_requests/{iid}/approvals"));
            (iid, has_reviewers, self.rest.get_json(&url).await)
        });
        let results: Vec<_> = futures::stream::iter(jobs)
            .buffer_unordered(PER_MR_CONCURRENCY)
            .collect()
            .await;
        let mut map = HashMap::new();
        for (iid, has_reviewers, res) in results {
            let approvals = res?;
            let approved = approvals["approved_by"]
                .as_array()
                .is_some_and(|a| !a.is_empty());
            let left = approvals["approvals_left"].as_u64().unwrap_or(0);
            let decision = if approved && left == 0 {
                Some("approved")
            } else if left > 0 || has_reviewers {
                Some("review_required")
            } else {
                None
            };
            if let Some(d) = decision {
                map.insert(iid, Some(d.to_string()));
            }
        }
        Ok(map)
    }

    async fn fetch_pull_reactions(&self) -> Result<HashMap<u64, u32>> {
        Ok(self
            .open_mrs()
            .await?
            .iter()
            .filter_map(|mr| Some((mr["iid"].as_u64()?, mr["upvotes"].as_u64()? as u32)))
            .collect())
    }

    async fn fetch_ci_statuses(&self) -> Result<HashMap<u64, Option<String>>> {
        // One pipelines page (newest first) mapped by commit SHA instead of
        // one call per MR.
        let pipelines = self
            .rest
            .get_json(&self.url("pipelines?per_page=100&order_by=id&sort=desc"))
            .await?;
        let mut by_sha: HashMap<String, &'static str> = HashMap::new();
        for p in pipelines.as_array().map(Vec::as_slice).unwrap_or_default() {
            if let (Some(sha), Some(bucket)) = (
                p["sha"].as_str(),
                p["status"].as_str().and_then(http::ci_bucket),
            ) {
                by_sha.entry(sha.to_string()).or_insert(bucket);
            }
        }
        Ok(self
            .open_mrs()
            .await?
            .iter()
            .filter_map(|mr| {
                let iid = mr["iid"].as_u64()?;
                let state = by_sha.get(mr["sha"].as_str()?)?;
                Some((iid, Some(state.to_string())))
            })
            .collect())
    }

    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        self.edit(
            "merge_requests",
            number,
            json!({ "add_labels": labels.join(",") }),
        )
        .await
    }

    async fn remove_pr_label(&self, number: u64, label: &str) -> Result<()> {
        self.edit("merge_requests", number, json!({ "remove_labels": label }))
            .await
    }

    async fn add_pr_assignees(&self, number: u64, assignees: &[String]) -> Result<()> {
        self.add_assignees_to("merge_requests", number, assignees)
            .await
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
        self.rest.require_auth("GitLab review")?;
        let mr = self
            .rest
            .get_json(&self.url(&format!("merge_requests/{pr_number}")))
            .await?;
        let refs = &mr["diff_refs"];
        let (Some(base), Some(start), Some(head)) = (
            refs["base_sha"].as_str(),
            refs["start_sha"].as_str(),
            refs["head_sha"].as_str(),
        ) else {
            anyhow::bail!("GitLab: MR !{pr_number} has no diff_refs (empty or still preparing)");
        };

        // Inline comments are diff-anchored discussions. A line outside the
        // diff is rejected by GitLab; keep it in the summary instead of
        // dropping it.
        let mut not_anchored = Vec::new();
        for (path, line, comment_body) in comments {
            let res = self
                .rest
                .send_json(
                    Method::POST,
                    &self.url(&format!("merge_requests/{pr_number}/discussions")),
                    &json!({
                        "body": ensure_comment_marker(comment_body, &self.marker),
                        "position": {
                            "position_type": "text",
                            "base_sha": base,
                            "start_sha": start,
                            "head_sha": head,
                            "old_path": path,
                            "new_path": path,
                            "new_line": line,
                        }
                    }),
                )
                .await;
            if let Err(e) = res {
                warn!("GitLab: inline comment on {path}:{line} rejected: {e:#}");
                not_anchored.push(format!("- `{path}:{line}` — {comment_body}"));
            }
        }
        let mut summary = body.to_string();
        if !not_anchored.is_empty() {
            summary.push_str("\n\n**Comments outside the diff:**\n");
            summary.push_str(&not_anchored.join("\n"));
        }
        self.rest
            .send_json(
                Method::POST,
                &self.url(&format!("merge_requests/{pr_number}/notes")),
                &json!({ "body": ensure_comment_marker(&summary, &self.marker) }),
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
        self.rest.require_auth("GitLab create MR")?;
        // Drafts are expressed by the "Draft:" title prefix.
        let title = if draft {
            format!("Draft: {title}")
        } else {
            title.to_string()
        };
        let resp = self
            .rest
            .send_json(
                Method::POST,
                &self.url("merge_requests"),
                &json!({
                    "title": title,
                    "description": body,
                    "source_branch": head,
                    "target_branch": base,
                }),
            )
            .await?;
        resp["iid"]
            .as_u64()
            .context("GitLab: missing iid in create MR response")
    }

    async fn find_comment_with_marker(
        &self,
        thread: Thread,
        marker: &str,
    ) -> Result<Option<CommentRef>> {
        let (kind, number) = Self::kind(thread);
        let mut found = None;
        self.list(&format!("{kind}/{number}/notes?sort=asc"), |note| {
            if note["system"].as_bool() == Some(true) {
                return true;
            }
            if http::has_marker(note["body"].as_str().unwrap_or(""), marker) {
                found = note["id"].as_u64();
                return found.is_none();
            }
            true
        })
        .await?;
        Ok(found.map(|id| CommentRef {
            thread,
            id,
            parent_id: None,
        }))
    }

    async fn update_comment(&self, comment: &CommentRef, body: &str) -> Result<()> {
        self.rest.require_auth("GitLab update note")?;
        let (kind, number) = Self::kind(comment.thread);
        self.rest
            .send_json(
                Method::PUT,
                &self.url(&format!("{kind}/{number}/notes/{}", comment.id)),
                &json!({ "body": body }),
            )
            .await?;
        Ok(())
    }

    async fn delete_comment(&self, comment: &CommentRef) -> Result<()> {
        self.rest.require_auth("GitLab delete note")?;
        let (kind, number) = Self::kind(comment.thread);
        self.rest
            .delete(&self.url(&format!("{kind}/{number}/notes/{}", comment.id)))
            .await
    }

    async fn upsert_tagged_comment(&self, number: u64, tag: &str, body: &str) -> Result<()> {
        self.rest.require_auth("GitLab comment")?;
        let me = self.self_username().await;
        let mut found = None;
        self.list(&format!("issues/{number}/notes?sort=asc"), |note| {
            if note["system"].as_bool() != Some(true)
                && super::tagged_comment_matches(
                    note["body"].as_str().unwrap_or(""),
                    note["author"]["username"].as_str().unwrap_or(""),
                    tag,
                    me.as_deref(),
                )
            {
                found = note["id"].as_u64();
            }
            found.is_none()
        })
        .await?;
        match super::tagged_target(found, me.as_deref()) {
            super::TaggedTarget::Refuse => anyhow::bail!(
                "a tagged comment exists on !{number} but the bot identity is unknown; not posting"
            ),
            super::TaggedTarget::Update(id) => {
                let existing = CommentRef {
                    thread: Thread::Issue(number),
                    id,
                    parent_id: None,
                };
                self.update_comment(&existing, body).await
            }
            super::TaggedTarget::Create => {
                self.rest
                    .send_json(
                        Method::POST,
                        &self.url(&format!("issues/{number}/notes")),
                        &json!({ "body": body }),
                    )
                    .await?;
                Ok(())
            }
        }
    }

    async fn fetch_comments_since(&self, since: &str) -> Result<Vec<PolledComment>> {
        // Project events, action=commented. `after` is a day (exclusive),
        // so ask from the previous day and filter precisely below.
        let after = http::parse_ts(since)
            .map(|d| {
                (d - chrono::Duration::days(1))
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .unwrap_or_default();
        let mut out = Vec::new();
        self.list(
            &format!("events?action=commented&target_type=note&sort=desc&after={after}"),
            |ev| {
                let created = ev["created_at"].as_str().unwrap_or("");
                if !http::ts_before(since, created) {
                    return false; // newest first: everything older is seen
                }
                let note = &ev["note"];
                if note["system"].as_bool() == Some(true) {
                    return true;
                }
                let is_pr = match note["noteable_type"].as_str() {
                    Some("MergeRequest") => true,
                    Some("Issue") => false,
                    _ => return true,
                };
                if let (Some(id), Some(number)) =
                    (note["id"].as_u64(), note["noteable_iid"].as_u64())
                {
                    out.push(PolledComment {
                        id,
                        number,
                        is_pr,
                        author: ev["author"]["username"]
                            .as_str()
                            .or_else(|| ev["author_username"].as_str())
                            .unwrap_or("")
                            .to_string(),
                        body: note["body"].as_str().unwrap_or("").to_string(),
                        created_at: created.to_string(),
                    });
                }
                true
            },
        )
        .await?;
        out.reverse();
        Ok(out)
    }

    async fn is_collaborator(&self, username: &str) -> Result<bool> {
        let Some(id) = self.user_id(username).await? else {
            return Ok(false);
        };
        // members/all includes inherited (group) memberships.
        let member = self
            .rest
            .get_json_opt(&self.url(&format!("members/all/{id}")))
            .await?;
        // 30 = Developer, 40 = Maintainer, 50 = Owner.
        Ok(member
            .and_then(|m| m["access_level"].as_u64())
            .is_some_and(|level| level >= 30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_provider::mock::{MockServer, Reply};

    fn provider(server: &MockServer) -> GitLabProvider {
        GitLabProvider::with_rest(
            Rest::new("GitLab", Auth::Header("Authorization", "Bearer tok".into())).unwrap(),
            &server.url(),
            "group/sub/proj",
            "<!-- wshm -->".into(),
        )
    }

    const P: &str = "/api/v4/projects/group%2Fsub%2Fproj";

    #[tokio::test]
    async fn lists_issues_with_pagination_and_normalizes_state() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/issues"),
            Reply::json(json!([
                {"iid": 1, "title": "a", "state": "opened", "labels": ["bug"],
                 "author": {"username": "alice"}, "upvotes": 2, "downvotes": 1,
                 "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-02T00:00:00Z"}
            ]))
            .header("x-next-page", "2")
            .when_query("&page=1"),
        );
        server.on(
            "GET",
            &format!("{P}/issues"),
            Reply::json(json!([{"iid": 2, "title": "b", "state": "closed"}])).when_query("&page=2"),
        );
        let issues = provider(&server)
            .fetch_all_issues(Some("2026-01-01T00:00:00Z"))
            .await
            .unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].state, "open");
        assert_eq!(issues[0].labels, vec!["bug"]);
        assert_eq!(issues[0].reactions_plus1, 2);
        assert_eq!(issues[0].reactions_total, 3);
        assert_eq!(issues[1].state, "closed");
        let reqs = server.requests();
        assert_eq!(
            reqs[0].header("authorization").as_deref(),
            Some("Bearer tok")
        );
        assert!(reqs[0].query.contains("state=all"));
        assert!(reqs[0]
            .query
            .contains("updated_after=2026-01-01T00%3A00%3A00Z"));
    }

    #[tokio::test]
    async fn labels_use_atomic_add_and_remove() {
        let server = MockServer::start().await;
        server.on(
            "PUT",
            &format!("{P}/issues/5"),
            Reply::json(json!({"iid": 5})),
        );
        server.on(
            "PUT",
            &format!("{P}/merge_requests/6"),
            Reply::json(json!({"iid": 6})),
        );
        let p = provider(&server);
        p.label_issue(5, &["bug".into(), "p1".into()])
            .await
            .unwrap();
        p.remove_pr_label(6, "wip").await.unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[0].json(), json!({"add_labels": "bug,p1"}));
        assert_eq!(reqs[1].path, format!("{P}/merge_requests/6"));
        assert_eq!(reqs[1].json(), json!({"remove_labels": "wip"}));
    }

    #[tokio::test]
    async fn comment_pr_updates_existing_marker_note() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/merge_requests/3/notes"),
            Reply::json(json!([
                {"id": 10, "body": "added 1 commit", "system": true},
                {"id": 11, "body": "old\n\n<!-- wshm -->", "system": false}
            ])),
        );
        server.on(
            "PUT",
            &format!("{P}/merge_requests/3/notes/11"),
            Reply::json(json!({})),
        );
        provider(&server).comment_pr(3, "new body").await.unwrap();
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1].method, "PUT");
        assert_eq!(reqs[1].json(), json!({"body": "new body\n\n<!-- wshm -->"}));
    }

    #[tokio::test]
    async fn comment_issue_creates_when_absent() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/issues/4/notes"),
            Reply::json(json!([])),
        );
        server.on(
            "POST",
            &format!("{P}/issues/4/notes"),
            Reply::json(json!({"id": 1})),
        );
        provider(&server).comment_issue(4, "hi").await.unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[1].method, "POST");
        assert_eq!(reqs[1].json(), json!({"body": "hi\n\n<!-- wshm -->"}));
    }

    #[tokio::test]
    async fn review_posts_positioned_discussions_with_diff_refs() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/merge_requests/7"),
            Reply::json(json!({"iid": 7, "diff_refs": {"base_sha": "b", "start_sha": "s", "head_sha": "h"}})),
        );
        server.on(
            "POST",
            &format!("{P}/merge_requests/7/discussions"),
            Reply::json(json!({})),
        );
        server.on(
            "POST",
            &format!("{P}/merge_requests/7/notes"),
            Reply::json(json!({})),
        );
        provider(&server)
            .submit_review(7, "summary", &[("src/a.rs".into(), 12, "nit".into())])
            .await
            .unwrap();
        let reqs = server.requests();
        let disc = reqs[1].json();
        assert_eq!(disc["position"]["head_sha"], "h");
        assert_eq!(disc["position"]["base_sha"], "b");
        assert_eq!(disc["position"]["start_sha"], "s");
        assert_eq!(disc["position"]["new_line"], 12);
        assert_eq!(disc["position"]["new_path"], "src/a.rs");
        assert_eq!(reqs[2].json(), json!({"body": "summary\n\n<!-- wshm -->"}));
        assert!(disc["body"].as_str().unwrap().ends_with("<!-- wshm -->"));
    }

    #[tokio::test]
    async fn review_keeps_rejected_inline_comments_in_summary() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/merge_requests/7"),
            Reply::json(json!({"diff_refs": {"base_sha": "b", "start_sha": "s", "head_sha": "h"}})),
        );
        server.on(
            "POST",
            &format!("{P}/merge_requests/7/discussions"),
            Reply::json(json!({"message": "line_code invalid"})).status(400),
        );
        server.on(
            "POST",
            &format!("{P}/merge_requests/7/notes"),
            Reply::json(json!({})),
        );
        provider(&server)
            .submit_review(7, "summary", &[("a.rs".into(), 99, "far".into())])
            .await
            .unwrap();
        let body = server.requests()[2].json()["body"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(body.contains("`a.rs:99` — far"), "{body}");
    }

    #[tokio::test]
    async fn collaborator_requires_developer_level() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            "/api/v4/users",
            Reply::json(json!([{"id": 42, "username": "bob"}])),
        );
        server.on(
            "GET",
            &format!("{P}/members/all/42"),
            Reply::json(json!({"access_level": 20})),
        );
        let p = provider(&server);
        assert!(!p.is_collaborator("bob").await.unwrap());

        let server = MockServer::start().await;
        server.on("GET", "/api/v4/users", Reply::json(json!([{"id": 42}])));
        server.on(
            "GET",
            &format!("{P}/members/all/42"),
            Reply::json(json!({"access_level": 30})),
        );
        assert!(provider(&server).is_collaborator("bob").await.unwrap());

        let server = MockServer::start().await;
        server.on("GET", "/api/v4/users", Reply::json(json!([{"id": 42}])));
        server.on(
            "GET",
            &format!("{P}/members/all/42"),
            Reply::json(json!({})).status(404),
        );
        assert!(!provider(&server).is_collaborator("bob").await.unwrap());

        assert!(provider(&server).is_collaborator("bad name").await.is_err());
    }

    #[tokio::test]
    async fn mr_mapping_and_diff_rendering() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/merge_requests/9"),
            Reply::json(json!({
                "iid": 9, "title": "t", "state": "merged", "sha": "h1",
                "diff_refs": {"base_sha": "b1"}, "source_branch": "f", "target_branch": "main",
                "detailed_merge_status": "mergeable", "has_conflicts": false,
                "head_pipeline": {"status": "failed"}, "upvotes": 3
            })),
        );
        server.on(
            "GET",
            &format!("{P}/merge_requests/9/diffs"),
            Reply::json(json!([
                {"old_path": "a", "new_path": "a", "diff": "@@ -1 +1 @@\n-x\n+y\n"},
                {"old_path": "n", "new_path": "n", "new_file": true, "diff": "@@ -0,0 +1 @@\n+z\n"}
            ])),
        );
        let p = provider(&server);
        let pr = p.fetch_pull(9).await.unwrap().unwrap();
        assert_eq!(pr.state, "closed");
        assert_eq!(pr.head_sha.as_deref(), Some("h1"));
        assert_eq!(pr.base_sha.as_deref(), Some("b1"));
        assert_eq!(pr.mergeable, Some(true));
        assert_eq!(pr.ci_status.as_deref(), Some("failure"));
        let diff = p.fetch_pr_diff(9).await.unwrap();
        assert!(diff.starts_with("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@"));
        assert!(diff.contains("--- /dev/null\n+++ b/n\n"));
    }

    #[tokio::test]
    async fn ci_statuses_map_pipelines_by_sha() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/pipelines"),
            Reply::json(json!([
                {"sha": "h1", "status": "running"},
                {"sha": "h1", "status": "failed"},
                {"sha": "h2", "status": "success"}
            ])),
        );
        server.on(
            "GET",
            &format!("{P}/merge_requests"),
            Reply::json(
                json!([{"iid": 1, "sha": "h1"}, {"iid": 2, "sha": "h2"}, {"iid": 3, "sha": "zz"}]),
            ),
        );
        let m = provider(&server).fetch_ci_statuses().await.unwrap();
        assert_eq!(m.get(&1), Some(&Some("pending".to_string())));
        assert_eq!(m.get(&2), Some(&Some("success".to_string())));
        assert!(!m.contains_key(&3));
    }

    #[tokio::test]
    async fn polls_comments_from_project_events() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/events"),
            Reply::json(json!([
                {"created_at": "2026-05-01T10:05:00Z", "author": {"username": "bob"},
                 "note": {"id": 2, "body": "/wshm triage", "noteable_type": "Issue", "noteable_iid": 4}},
                {"created_at": "2026-05-01T10:01:00Z", "author": {"username": "eve"},
                 "note": {"id": 1, "body": "/wshm analyze", "noteable_type": "MergeRequest", "noteable_iid": 8}},
                {"created_at": "2026-05-01T09:00:00Z", "author": {"username": "old"},
                 "note": {"id": 0, "body": "old", "noteable_type": "Issue", "noteable_iid": 1}}
            ])),
        );
        let c = provider(&server)
            .fetch_comments_since("2026-05-01T10:00:00Z")
            .await
            .unwrap();
        assert_eq!(c.len(), 2);
        assert_eq!(
            (c[0].number, c[0].is_pr, c[0].author.as_str()),
            (8, true, "eve")
        );
        assert_eq!((c[1].number, c[1].is_pr), (4, false));
        assert!(server.requests()[0].query.contains("after=2026-04-30"));
    }

    #[tokio::test]
    async fn anonymous_provider_refuses_mutations() {
        let server = MockServer::start().await;
        let p = GitLabProvider::with_rest(
            Rest::new("GitLab", Auth::None).unwrap(),
            &server.url(),
            "g/p",
            "<!-- wshm -->".into(),
        );
        assert!(p.label_issue(1, &["x".into()]).await.is_err());
        assert!(server.requests().is_empty());
    }
}
