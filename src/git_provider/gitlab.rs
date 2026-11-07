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
use super::{Capability, CommentRef, GitProvider, MergeOutcome, MergedPr, PolledComment, Thread};

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

/// GitLab bot accounts: project / group access tokens
/// (`project_<id>_bot_<hash>`, `group_<id>_bot...`), service accounts and
/// the ghost user. They never count as approvers.
fn is_bot_username(username: &str) -> bool {
    let u = username.to_ascii_lowercase();
    u.is_empty()
        || u == "ghost"
        || u.ends_with("[bot]")
        || u.ends_with("_bot")
        || u.contains("_bot_")
        || u.starts_with("service_account_")
}

/// Extra marker on review notes (summary and inline): keeps them apart from
/// the analysis comment that [`GitProvider::find_comment_with_marker`]
/// updates in place, since GitLab lists every MR note in one thread.
fn review_marker(marker: &str) -> String {
    let name = marker
        .trim()
        .trim_start_matches("<!--")
        .trim_end_matches("-->")
        .trim();
    format!("<!-- {name}:review -->")
}

/// GitHub's ```suggestion fence replaces the commented line; GitLab's
/// equivalent spells the range explicitly (`-0+0` = that line only).
fn gitlab_suggestions(body: &str) -> String {
    body.lines()
        .map(|l| {
            if l.trim_end() == "```suggestion" {
                "```suggestion:-0+0"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Old path and, for an unchanged line, old line number of `new_line` in
/// `path`, read from the MR diff entries. `(path, None)` for an added line
/// or when the line is not in the diff (GitLab then rejects the position).
fn old_side(diffs: &[Value], path: &str, new_line: u64) -> (String, Option<u64>) {
    let Some(entry) = diffs.iter().find(|d| d["new_path"].as_str() == Some(path)) else {
        return (path.to_string(), None);
    };
    let old_path = entry["old_path"].as_str().unwrap_or(path).to_string();
    let (mut old, mut new) = (0u64, 0u64);
    for l in entry["diff"].as_str().unwrap_or("").lines() {
        if let Some(rest) = l.strip_prefix("@@ ") {
            // @@ -a[,b] +c[,d] @@
            let mut parts = rest.split_whitespace();
            let start = |p: Option<&str>| {
                p.and_then(|s| s.get(1..)?.split(',').next()?.parse::<u64>().ok())
                    .unwrap_or(1)
            };
            old = start(parts.next());
            new = start(parts.next());
            continue;
        }
        match l.chars().next() {
            Some('+') => {
                if new == new_line {
                    return (old_path, None);
                }
                new += 1;
            }
            Some('-') => old += 1,
            Some('\\') => {}
            _ => {
                if new == new_line {
                    return (old_path, Some(old));
                }
                old += 1;
                new += 1;
            }
        }
    }
    (old_path, None)
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
            | Capability::CommentPolling
            | Capability::CommitStatus
            | Capability::Merge
            // Counted only where approvals reset on push (see
            // fetch_pr_approvals): GitLab binds them to no commit.
            | Capability::PrApprovals => true,
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

    async fn set_commit_status(&self, sha: &str, status: &super::CommitStatus) -> Result<()> {
        self.rest.require_auth("set commit status")?;
        let state = match status.state {
            super::CommitState::Pending => "pending",
            super::CommitState::Success => "success",
            super::CommitState::Failure => "failed",
        };
        self.rest
            .send_json(
                Method::POST,
                &self.url(&format!("statuses/{}", urlencoding::encode(sha))),
                &json!({
                    "state": state,
                    "name": status.context,
                    "description": super::status_description(&status.description),
                }),
            )
            .await?;
        Ok(())
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

        // A position on an unchanged (context) line needs its old line too,
        // and a renamed file its old path: both come from the MR diffs.
        let diffs = if comments.is_empty() {
            Vec::new()
        } else {
            self.list_all(&format!("merge_requests/{pr_number}/diffs"))
                .await
                .unwrap_or_else(|e| {
                    warn!("GitLab: MR !{pr_number} diffs unavailable, anchoring on new lines only: {e:#}");
                    Vec::new()
                })
        };
        let review_marker = review_marker(&self.marker);

        // Inline comments are diff-anchored discussions. A line outside the
        // diff is rejected by GitLab; keep it in the summary instead of
        // dropping it.
        let mut not_anchored = Vec::new();
        for (path, line, comment_body) in comments {
            let (old_path, old_line) = old_side(&diffs, path, *line);
            let mut position = json!({
                "position_type": "text",
                "base_sha": base,
                "start_sha": start,
                "head_sha": head,
                "old_path": old_path,
                "new_path": path,
                "new_line": line,
            });
            if let Some(old) = old_line {
                position["old_line"] = json!(old);
            }
            let note = format!(
                "{}\n{review_marker}",
                ensure_comment_marker(&gitlab_suggestions(comment_body), &self.marker)
            );
            let res = self
                .rest
                .send_json(
                    Method::POST,
                    &self.url(&format!("merge_requests/{pr_number}/discussions")),
                    &json!({ "body": note, "position": position }),
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
                &json!({
                    "body": format!(
                        "{}\n{review_marker}",
                        ensure_comment_marker(&summary, &self.marker)
                    )
                }),
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

    async fn merge_pr(&self, number: u64, sha: Option<&str>, squash: bool) -> Result<MergeOutcome> {
        self.rest.require_auth("GitLab merge")?;
        let mut body = json!({ "squash": squash });
        if let Some(sha) = sha {
            // GitLab answers 409 when the source branch moved past `sha`.
            body["sha"] = json!(sha);
        }
        let url = self.url(&format!("merge_requests/{number}/merge"));
        // Not re-issued after a 5xx: the merge may have happened anyway.
        // The MR state then says what really happened.
        let mr = match self.rest.send_json_once(Method::PUT, &url, &body).await {
            Ok(mr) => mr,
            Err(e) => {
                let live = self
                    .rest
                    .get_json(&self.url(&format!("merge_requests/{number}")))
                    .await;
                if let Ok(live) = live {
                    if live["state"].as_str() == Some("merged") {
                        warn!("GitLab: merge of MR !{number} answered an error but the MR is merged: {e:#}");
                        return Ok(MergeOutcome::Merged);
                    }
                }
                return Err(e)
                    .with_context(|| format!("GitLab: merging MR !{number} (not merged)"));
            }
        };
        Ok(if mr["state"].as_str() == Some("merged") {
            MergeOutcome::Merged
        } else {
            MergeOutcome::NotMerged
        })
    }

    async fn fetch_pr_approvers(&self, number: u64) -> Result<Vec<String>> {
        let approvals = self
            .rest
            .get_json(&self.url(&format!("merge_requests/{number}/approvals")))
            .await?;
        Ok(approvals["approved_by"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|a| &a["user"])
            .filter(|u| {
                u["bot"].as_bool() != Some(true)
                    && !is_bot_username(u["username"].as_str().unwrap_or(""))
            })
            .filter_map(|u| u["username"].as_str().map(String::from))
            .collect())
    }

    async fn fetch_pr_approvals(&self, number: u64) -> Result<Vec<super::PrApproval>> {
        // GitLab approvals name no commit. They are only safe to count
        // when the project drops them on every push (they then concern the
        // head); otherwise an approval of an older commit would count, so
        // this errs and the approval step reports them as not counted.
        let settings = self.rest.get_json(&self.url("approvals")).await?;
        if settings["reset_approvals_on_push"].as_bool() != Some(true) {
            anyhow::bail!(
                "approvals are kept on push (enable \"Remove all approvals when commits are added\")"
            );
        }
        let mut out = Vec::new();
        for login in self.fetch_pr_approvers(number).await? {
            // Developer or above, like the merge gate.
            let write_access = self.is_collaborator(&login).await.unwrap_or(false);
            out.push(super::PrApproval {
                login,
                commit_sha: None,
                bot: false,
                write_access,
            });
        }
        Ok(out)
    }

    async fn find_wshm_comments(&self, thread: Thread, marker: &str) -> Result<Vec<CommentRef>> {
        // Unlike find_comment_with_marker: review notes and inline
        // discussions too (they are wshm's), system notes never.
        let (kind, number) = Self::kind(thread);
        let mut out = Vec::new();
        self.list(&format!("{kind}/{number}/notes?sort=asc"), |note| {
            if note["system"].as_bool() != Some(true)
                && http::has_marker(note["body"].as_str().unwrap_or(""), marker)
            {
                if let Some(id) = note["id"].as_u64() {
                    out.push(CommentRef {
                        thread,
                        id,
                        parent_id: None,
                    });
                }
            }
            true
        })
        .await?;
        Ok(out)
    }

    async fn find_comment_with_marker(
        &self,
        thread: Thread,
        marker: &str,
    ) -> Result<Option<CommentRef>> {
        let (kind, number) = Self::kind(thread);
        let mut found = None;
        let review = review_marker(&self.marker);
        self.list(&format!("{kind}/{number}/notes?sort=asc"), |note| {
            // System notes, diff-anchored notes and review summaries are
            // never the comment wshm updates in place (they carry the marker
            // for the self-comment guard only).
            let body = note["body"].as_str().unwrap_or("");
            if note["system"].as_bool() == Some(true)
                || note["type"].as_str() == Some("DiffNote")
                || body.contains(&review)
            {
                return true;
            }
            if http::has_marker(body, marker) {
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

    async fn fetch_repo_private(&self) -> Result<bool> {
        let project = self
            .rest
            .get_json(&format!("{}/projects/{}", self.api, self.project))
            .await?;
        match project["visibility"].as_str() {
            Some(v) => Ok(v != "public"),
            None => anyhow::bail!("GitLab: project has no 'visibility' field"),
        }
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
    async fn incremental_mr_sync_follows_pages_and_stops_at_since() {
        let server = MockServer::start().await;
        let mr = |iid: u64, state: &str, updated: &str| {
            json!({"iid": iid, "title": "t", "state": state, "sha": format!("h{iid}"),
                   "source_branch": "f", "target_branch": "main", "has_conflicts": iid == 2,
                   "detailed_merge_status": "mergeable", "updated_at": updated})
        };
        server.on(
            "GET",
            &format!("{P}/merge_requests"),
            Reply::json(json!([mr(3, "opened", "2026-03-03T00:00:00Z")]))
                .header("x-next-page", "2")
                .when_query("&page=1"),
        );
        server.on(
            "GET",
            &format!("{P}/merge_requests"),
            Reply::json(json!([
                mr(2, "merged", "2026-03-02T00:00:00Z"),
                mr(1, "closed", "2026-02-01T00:00:00Z")
            ]))
            .header("x-next-page", "3")
            .when_query("&page=2"),
        );
        let prs = provider(&server)
            .fetch_pulls_incremental("all", Some("2026-03-01T00:00:00Z"))
            .await
            .unwrap();
        // MR 1 is older than `since`: listing stops there (no page 3).
        assert_eq!(prs.iter().map(|p| p.number).collect::<Vec<_>>(), vec![3, 2]);
        assert_eq!(
            (prs[0].state.as_str(), prs[1].state.as_str()),
            ("open", "closed")
        );
        assert_eq!(prs[0].mergeable, Some(true));
        assert_eq!(prs[1].mergeable, Some(false));
        assert_eq!(prs[0].head_ref.as_deref(), Some("f"));
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2);
        assert!(
            reqs[0].query.contains("state=all") && reqs[0].query.contains("order_by=updated_at")
        );
        assert!(reqs[0]
            .query
            .contains("updated_after=2026-03-01T00%3A00%3A00Z"));
        assert!(reqs[1].query.contains("page=2"));
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
        // src/a.rs: line 12 is added; line 14 is unchanged (old line 13).
        // src/new.rs was renamed from src/old.rs.
        server.on(
            "GET",
            &format!("{P}/merge_requests/7/diffs"),
            Reply::json(json!([
                {"old_path": "src/a.rs", "new_path": "src/a.rs",
                 "diff": "@@ -10,4 +10,5 @@ fn x() {\n ctx10\n ctx11\n+added12\n ctx13\n ctx14\n"},
                {"old_path": "src/old.rs", "new_path": "src/new.rs", "renamed_file": true,
                 "diff": "@@ -1 +1 @@\n-a\n+b\n"}
            ])),
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
        let suggestion = "nit\n\n```suggestion\nlet y = 2;\n```";
        provider(&server)
            .submit_review(
                7,
                "summary",
                &[
                    ("src/a.rs".into(), 12, suggestion.into()),
                    ("src/a.rs".into(), 14, "ctx".into()),
                    ("src/new.rs".into(), 1, "renamed".into()),
                ],
            )
            .await
            .unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[1].path, format!("{P}/merge_requests/7/diffs"));

        let added = reqs[2].json();
        let pos = &added["position"];
        assert_eq!(
            (
                pos["base_sha"].as_str(),
                pos["start_sha"].as_str(),
                pos["head_sha"].as_str()
            ),
            (Some("b"), Some("s"), Some("h"))
        );
        assert_eq!(pos["position_type"], "text");
        assert_eq!(pos["new_path"], "src/a.rs");
        assert_eq!(pos["old_path"], "src/a.rs");
        assert_eq!(pos["new_line"], 12);
        assert!(pos.get("old_line").is_none(), "{pos}");
        let body = added["body"].as_str().unwrap();
        assert!(
            body.contains("```suggestion:-0+0\nlet y = 2;\n```"),
            "{body}"
        );
        assert!(body.contains("<!-- wshm -->") && body.ends_with("<!-- wshm:review -->"));

        // Context line: both sides.
        let ctx = &reqs[3].json()["position"];
        assert_eq!(
            (ctx["new_line"].as_u64(), ctx["old_line"].as_u64()),
            (Some(14), Some(13))
        );
        // Renamed file: old path from the diff.
        let renamed = &reqs[4].json()["position"];
        assert_eq!(renamed["old_path"], "src/old.rs");
        assert_eq!(renamed["new_path"], "src/new.rs");

        assert_eq!(reqs[5].path, format!("{P}/merge_requests/7/notes"));
        assert_eq!(
            reqs[5].json(),
            json!({"body": "summary\n\n<!-- wshm -->\n<!-- wshm:review -->"})
        );
    }

    #[tokio::test]
    async fn revert_lookup_finds_review_and_inline_notes() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/merge_requests/3/notes"),
            Reply::json(json!([
                {"id": 19, "system": true, "body": "<!-- wshm -->"},
                {"id": 20, "type": "DiffNote", "body": "inline\n\n<!-- wshm -->\n<!-- wshm:review -->"},
                {"id": 21, "body": "review\n\n<!-- wshm -->\n<!-- wshm:review -->"},
                {"id": 22, "body": "analysis\n\n<!-- wshm -->"},
                {"id": 23, "body": "a human"}
            ])),
        );
        let found = provider(&server)
            .find_wshm_comments(Thread::Pull(3), "<!-- wshm -->")
            .await
            .unwrap();
        assert_eq!(
            found.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![20, 21, 22]
        );
    }

    #[tokio::test]
    async fn pipeline_approvals_only_when_reset_on_push() {
        let approvals = |server: &MockServer| {
            server.on(
                "GET",
                &format!("{P}/merge_requests/4/approvals"),
                Reply::json(json!({"approved_by": [
                    {"user": {"username": "bob"}},
                    {"user": {"username": "svc", "bot": true}}
                ]})),
            );
        };
        // Approvals survive pushes: not counted (an error, never a count).
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/approvals"),
            Reply::json(json!({"reset_approvals_on_push": false})),
        );
        approvals(&server);
        assert!(provider(&server).fetch_pr_approvals(4).await.is_err());

        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/approvals"),
            Reply::json(json!({"reset_approvals_on_push": true})),
        );
        approvals(&server);
        server.on("GET", "/api/v4/users", Reply::json(json!([{"id": 42}])));
        server.on(
            "GET",
            &format!("{P}/members/all/42"),
            Reply::json(json!({"access_level": 30})),
        );
        let a = provider(&server).fetch_pr_approvals(4).await.unwrap();
        assert_eq!(
            a,
            vec![super::super::PrApproval {
                login: "bob".into(),
                commit_sha: None,
                bot: false,
                write_access: true,
            }]
        );
    }

    #[tokio::test]
    async fn approvers_leave_out_bots() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/merge_requests/4/approvals"),
            Reply::json(json!({"approved_by": [
                {"user": {"username": "bob"}},
                {"user": {"username": "project_9_bot_ab12"}},
                {"user": {"username": "group_3_bot"}},
                {"user": {"username": "svc", "bot": true}}
            ]})),
        );
        assert_eq!(
            provider(&server).fetch_pr_approvers(4).await.unwrap(),
            vec!["bob"]
        );
    }

    #[tokio::test]
    async fn analysis_comment_never_overwrites_review_notes() {
        let server = MockServer::start().await;
        server.on(
            "GET",
            &format!("{P}/merge_requests/3/notes"),
            Reply::json(json!([
                {"id": 20, "type": "DiffNote", "body": "inline\n\n<!-- wshm -->"},
                {"id": 21, "body": "review\n\n<!-- wshm -->\n<!-- wshm:review -->"}
            ])),
        );
        server.on(
            "POST",
            &format!("{P}/merge_requests/3/notes"),
            Reply::json(json!({"id": 22})),
        );
        provider(&server).comment_pr(3, "analysis").await.unwrap();
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[1].method, "POST");
        assert_eq!(reqs[1].json(), json!({"body": "analysis\n\n<!-- wshm -->"}));
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
        let body = server.requests()[3].json()["body"]
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

    #[tokio::test]
    async fn commit_status_uses_gitlab_states() {
        let server = MockServer::start().await;
        server.on("POST", &format!("{P}/statuses/abc"), Reply::json(json!({})));
        let p = provider(&server);
        assert!(p.supports(Capability::CommitStatus));
        assert!(p.supports(Capability::PrApprovals));
        p.set_commit_status(
            "abc",
            &crate::git_provider::CommitStatus {
                state: crate::git_provider::CommitState::Failure,
                context: "wshm/review".into(),
                description: "x".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            server.requests()[0].json(),
            json!({"state": "failed", "name": "wshm/review", "description": "x"})
        );
    }
}
