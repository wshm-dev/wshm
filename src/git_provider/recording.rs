//! Test-only forge that records every call (no network). Used by the
//! pipeline-level injection tests to assert which mutating actions ran.

use std::collections::HashSet;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;

use super::{Capability, CommentRef, GitProvider, MergedPr, Thread};
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;

#[derive(Default)]
pub struct RecordingForge {
    pub calls: Mutex<Vec<String>>,
    pub collaborators: HashSet<String>,
    pub diff: String,
    /// Capabilities the forge claims.
    pub caps: Vec<Capability>,
    /// Returned by `fetch_pr_approvals`.
    pub approvals: Vec<super::PrApproval>,
}

impl RecordingForge {
    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Calls whose name starts with one of `names`.
    pub fn any_of(&self, names: &[&str]) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| names.iter().any(|n| c.starts_with(n)))
            .collect()
    }
}

#[async_trait]
impl GitProvider for RecordingForge {
    fn provider_name(&self) -> &str {
        "test"
    }
    fn repo_slug(&self) -> String {
        "o/r".into()
    }
    fn comment_marker(&self) -> &str {
        "<!-- wshm -->"
    }
    fn is_authenticated(&self) -> bool {
        true
    }
    fn supports(&self, cap: Capability) -> bool {
        self.caps.contains(&cap)
    }
    async fn set_commit_status(&self, sha: &str, status: &super::CommitStatus) -> Result<()> {
        self.log(format!(
            "set_commit_status {sha} {} {} {}",
            status.context,
            status.state.as_str(),
            status.description
        ));
        Ok(())
    }
    async fn fetch_pr_approvals(&self, _number: u64) -> Result<Vec<super::PrApproval>> {
        Ok(self.approvals.clone())
    }
    async fn fetch_issues(&self, _since: Option<&str>) -> Result<Vec<Issue>> {
        Ok(Vec::new())
    }
    async fn fetch_all_issues(&self, _since: Option<&str>) -> Result<Vec<Issue>> {
        Ok(Vec::new())
    }
    async fn fetch_issue(&self, _number: u64) -> Result<Option<Issue>> {
        Ok(None)
    }
    async fn create_issue(&self, title: &str, _body: &str, _labels: &[String]) -> Result<u64> {
        self.log(format!("create_issue {title}"));
        Ok(1000)
    }
    async fn close_issue(&self, number: u64) -> Result<()> {
        self.log(format!("close_issue {number}"));
        Ok(())
    }
    async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        self.log(format!("label_issue {number} {labels:?}"));
        Ok(())
    }
    async fn remove_label(&self, number: u64, label: &str) -> Result<()> {
        self.log(format!("remove_label {number} {label}"));
        Ok(())
    }
    async fn add_assignees(&self, number: u64, _assignees: &[String]) -> Result<()> {
        self.log(format!("add_assignees {number}"));
        Ok(())
    }
    async fn comment_issue(&self, number: u64, body: &str) -> Result<()> {
        self.log(format!("comment_issue {number} {body}"));
        Ok(())
    }
    async fn fetch_pulls(&self) -> Result<Vec<PullRequest>> {
        Ok(Vec::new())
    }
    async fn fetch_pulls_incremental(
        &self,
        _state: &str,
        _since: Option<&str>,
    ) -> Result<Vec<PullRequest>> {
        Ok(Vec::new())
    }
    async fn fetch_pull(&self, _number: u64) -> Result<Option<PullRequest>> {
        Ok(None)
    }
    async fn fetch_pr_mergeable(&self, _number: u64) -> Result<Option<bool>> {
        Ok(None)
    }
    async fn fetch_pr_diff(&self, _number: u64) -> Result<String> {
        Ok(self.diff.clone())
    }
    async fn fetch_merged_pulls(&self, _since: Option<&str>) -> Result<Vec<MergedPr>> {
        Ok(Vec::new())
    }
    async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        self.log(format!("label_pr {number} {labels:?}"));
        Ok(())
    }
    async fn remove_pr_label(&self, number: u64, label: &str) -> Result<()> {
        self.log(format!("remove_pr_label {number} {label}"));
        Ok(())
    }
    async fn add_pr_assignees(&self, number: u64, _assignees: &[String]) -> Result<()> {
        self.log(format!("add_pr_assignees {number}"));
        Ok(())
    }
    async fn comment_pr(&self, number: u64, body: &str) -> Result<()> {
        self.log(format!("comment_pr {number} {body}"));
        Ok(())
    }
    async fn submit_review(
        &self,
        pr_number: u64,
        _body: &str,
        comments: &[(String, u64, String)],
    ) -> Result<()> {
        self.log(format!("submit_review {pr_number} {}", comments.len()));
        Ok(())
    }
    async fn create_pr(
        &self,
        title: &str,
        _body: &str,
        _head: &str,
        _base: &str,
        _draft: bool,
    ) -> Result<u64> {
        self.log(format!("create_pr {title}"));
        Ok(2000)
    }
    async fn find_comment_with_marker(
        &self,
        _thread: Thread,
        _marker: &str,
    ) -> Result<Option<CommentRef>> {
        Ok(None)
    }
    async fn update_comment(&self, comment: &CommentRef, _body: &str) -> Result<()> {
        self.log(format!("update_comment {comment}"));
        Ok(())
    }
    async fn delete_comment(&self, comment: &CommentRef) -> Result<()> {
        self.log(format!("delete_comment {comment}"));
        Ok(())
    }
    async fn is_collaborator(&self, username: &str) -> Result<bool> {
        Ok(self.collaborators.contains(username))
    }
}
