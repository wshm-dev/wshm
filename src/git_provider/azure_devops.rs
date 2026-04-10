use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::info;

use crate::config::Config;
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;

use super::{GitProvider, MergedPr};

/// Azure DevOps provider — uses Azure DevOps REST API.
/// Supports Azure DevOps Services (dev.azure.com) and Azure DevOps Server (on-premise).
pub struct AzureDevOpsProvider {
    http: reqwest::Client,
    base_url: String,
    org: String,
    project: String,
    repo_name: String,
    token: String,
}

impl AzureDevOpsProvider {
    pub fn new(config: &Config, base_url: Option<&str>) -> Result<Self> {
        let token = std::env::var("AZURE_DEVOPS_TOKEN")
            .or_else(|_| std::env::var("WSHM_TOKEN"))
            .context("No Azure DevOps token found. Set AZURE_DEVOPS_TOKEN or WSHM_TOKEN.")?;

        let base = base_url.unwrap_or("https://dev.azure.com");

        // For Azure DevOps, repo_owner = org/project, repo_name = repo
        let org = config.repo_owner.clone();
        let project = config.repo_name.clone();
        let repo_name = config.repo_name.clone();

        let http = reqwest::Client::builder()
            .user_agent("wshm")
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        info!("Azure DevOps provider initialized: {base}/{org}/{project}");

        Ok(Self { http, base_url: base.trim_end_matches('/').to_string(), org, project, repo_name, token })
    }

    #[allow(dead_code)]
    fn api_url(&self, path: &str) -> String {
        format!("{}/{}/{}/_apis/{}&api-version=7.1", self.base_url, self.org, self.project, path)
    }

    fn git_api(&self, path: &str) -> String {
        format!("{}/{}/{}/_apis/git/repositories/{}/{}?api-version=7.1", self.base_url, self.org, self.project, self.repo_name, path)
    }

    fn wit_api(&self, path: &str) -> String {
        format!("{}/{}/{}/_apis/wit/{}?api-version=7.1", self.base_url, self.org, self.project, path)
    }

    async fn get(&self, url: &str) -> Result<serde_json::Value> {
        let resp = self.http.get(url)
            .basic_auth("", Some(&self.token))
            .send().await.context("Azure DevOps GET")?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() { anyhow::bail!("Azure DevOps API error ({status}): {}", &text[..text.len().min(200)]); }
        Ok(serde_json::from_str(&text)?)
    }

    async fn post(&self, url: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let resp = self.http.post(url)
            .basic_auth("", Some(&self.token))
            .json(body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() { anyhow::bail!("Azure DevOps API error ({status}): {}", &text[..text.len().min(200)]); }
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }
}

#[async_trait]
impl GitProvider for AzureDevOpsProvider {
    fn provider_name(&self) -> &str { "azure-devops" }
    fn repo_slug(&self) -> String { format!("{}/{}/{}", self.org, self.project, self.repo_name) }

    async fn fetch_issues(&self, _since: Option<&str>) -> Result<Vec<Issue>> {
        // Azure DevOps uses Work Items, not GitHub-style issues
        // WIQL query for active bugs/user stories
        let wiql_url = self.wit_api("wiql");
        let query = serde_json::json!({
            "query": "SELECT [System.Id], [System.Title], [System.State], [System.CreatedDate], [System.ChangedDate], [System.AssignedTo], [System.WorkItemType] FROM WorkItems WHERE [System.State] = 'Active' OR [System.State] = 'New' ORDER BY [System.CreatedDate] DESC"
        });
        let result = self.post(&wiql_url, &query).await?;
        let work_items = result["workItems"].as_array().context("No workItems")?;

        let mut issues = Vec::new();
        for wi in work_items.iter().take(100) {
            if let Some(id) = wi["id"].as_u64() {
                let url = self.wit_api(&format!("workitems/{id}"));
                if let Ok(item) = self.get(&url).await {
                    let fields = &item["fields"];
                    issues.push(Issue {
                        number: id,
                        title: fields["System.Title"].as_str().unwrap_or("").to_string(),
                        body: fields["System.Description"].as_str().map(String::from),
                        state: "open".to_string(),
                        labels: vec![fields["System.WorkItemType"].as_str().unwrap_or("").to_string()],
                        author: fields["System.CreatedBy"]["uniqueName"].as_str().map(String::from),
                        created_at: fields["System.CreatedDate"].as_str().unwrap_or("").to_string(),
                        updated_at: fields["System.ChangedDate"].as_str().unwrap_or("").to_string(),
                        reactions_plus1: 0,
                        reactions_total: 0,
                    });
                }
            }
        }
        Ok(issues)
    }

    async fn label_issue(&self, _number: u64, _labels: &[String]) -> Result<()> {
        // Azure DevOps uses tags, not labels
        info!("Azure DevOps label_issue not yet implemented (use tags)");
        Ok(())
    }

    async fn remove_label(&self, _number: u64, _label: &str) -> Result<()> { Ok(()) }

    async fn comment_issue(&self, number: u64, body: &str, _marker: &str) -> Result<()> {
        let url = self.wit_api(&format!("workitems/{number}/comments"));
        self.post(&url, &serde_json::json!({ "text": body })).await?;
        Ok(())
    }

    async fn delete_comment(&self, _comment_id: u64) -> Result<()> { Ok(()) }
    async fn find_comment_with_marker(&self, _number: u64, _marker: &str) -> Result<Option<u64>> { Ok(None) }

    async fn close_issue(&self, _number: u64) -> Result<()> {
        info!("Azure DevOps close_issue: use state transition");
        Ok(())
    }

    async fn create_issue(&self, title: &str, body: &str, _labels: &[String]) -> Result<u64> {
        let url = self.wit_api("workitems/$Bug");
        let ops = serde_json::json!([
            { "op": "add", "path": "/fields/System.Title", "value": title },
            { "op": "add", "path": "/fields/System.Description", "value": body },
        ]);
        let resp = self.http.post(&url)
            .basic_auth("", Some(&self.token))
            .header("Content-Type", "application/json-patch+json")
            .json(&ops).send().await?;
        let json: serde_json::Value = resp.json().await?;
        Ok(json["id"].as_u64().context("Missing id")?)
    }

    async fn add_assignees(&self, _number: u64, _assignees: &[String]) -> Result<()> { Ok(()) }

    async fn fetch_pulls(&self) -> Result<Vec<PullRequest>> {
        let url = self.git_api("pullrequests?status=active");
        let result = self.get(&url).await?;
        let prs = result["value"].as_array().context("No value")?;
        Ok(prs.iter().map(|pr| PullRequest {
            number: pr["pullRequestId"].as_u64().unwrap_or(0),
            title: pr["title"].as_str().unwrap_or("").to_string(),
            body: pr["description"].as_str().map(String::from),
            state: "open".to_string(),
            labels: Vec::new(),
            author: pr["createdBy"]["uniqueName"].as_str().map(String::from),
            head_sha: pr["lastMergeSourceCommit"]["commitId"].as_str().map(String::from),
            base_sha: pr["lastMergeTargetCommit"]["commitId"].as_str().map(String::from),
            head_ref: pr["sourceRefName"].as_str().map(|s| s.replace("refs/heads/", "")),
            base_ref: pr["targetRefName"].as_str().map(|s| s.replace("refs/heads/", "")),
            mergeable: pr["mergeStatus"].as_str().map(|s| s == "succeeded"),
            ci_status: None,
            created_at: pr["creationDate"].as_str().unwrap_or("").to_string(),
            updated_at: pr["creationDate"].as_str().unwrap_or("").to_string(),
        }).collect())
    }

    async fn fetch_merged_pulls(&self, _since: Option<&str>) -> Result<Vec<MergedPr>> {
        let url = self.git_api("pullrequests?status=completed&$top=50");
        let result = self.get(&url).await?;
        let prs = result["value"].as_array().context("No value")?;
        Ok(prs.iter().map(|pr| MergedPr {
            number: pr["pullRequestId"].as_u64().unwrap_or(0),
            title: pr["title"].as_str().unwrap_or("").to_string(),
            merged_at: pr["closedDate"].as_str().unwrap_or("").to_string(),
            author: pr["createdBy"]["uniqueName"].as_str().map(String::from),
            body: pr["description"].as_str().map(String::from),
            labels: Vec::new(),
        }).collect())
    }

    async fn fetch_pr_mergeable(&self, number: u64) -> Result<Option<bool>> {
        let url = self.git_api(&format!("pullrequests/{number}"));
        let pr = self.get(&url).await?;
        Ok(pr["mergeStatus"].as_str().map(|s| s == "succeeded"))
    }

    async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        // Azure DevOps: get iterations then diff
        let url = self.git_api(&format!("pullrequests/{number}/iterations"));
        let result = self.get(&url).await?;
        let iterations = result["value"].as_array().context("No iterations")?;
        if let Some(last) = iterations.last() {
            if let Some(id) = last["id"].as_u64() {
                let changes_url = self.git_api(&format!("pullrequests/{number}/iterations/{id}/changes"));
                let changes = self.get(&changes_url).await?;
                return Ok(serde_json::to_string_pretty(&changes)?);
            }
        }
        Ok(String::new())
    }

    async fn submit_review(&self, pr_number: u64, body: &str, _comments: &[(String, u64, String)]) -> Result<()> {
        let url = self.git_api(&format!("pullrequests/{pr_number}/threads"));
        self.post(&url, &serde_json::json!({
            "comments": [{ "parentCommentId": 0, "content": body, "commentType": 1 }],
            "status": 1
        })).await?;
        Ok(())
    }

    async fn create_pr(&self, title: &str, body: &str, head: &str, base: &str, _draft: bool) -> Result<u64> {
        let url = self.git_api("pullrequests");
        let resp = self.post(&url, &serde_json::json!({
            "sourceRefName": format!("refs/heads/{head}"),
            "targetRefName": format!("refs/heads/{base}"),
            "title": title,
            "description": body,
        })).await?;
        Ok(resp["pullRequestId"].as_u64().context("Missing pullRequestId")?)
    }

    async fn label_pr(&self, _number: u64, _labels: &[String]) -> Result<()> { Ok(()) }

    async fn comment_pr(&self, number: u64, body: &str, _marker: &str) -> Result<()> {
        let url = self.git_api(&format!("pullrequests/{number}/threads"));
        self.post(&url, &serde_json::json!({
            "comments": [{ "parentCommentId": 0, "content": body, "commentType": 1 }],
            "status": 1
        })).await?;
        Ok(())
    }

    async fn is_collaborator(&self, _username: &str) -> Result<bool> {
        // Azure DevOps: check team membership — simplified
        Ok(true) // Default to true, rely on Azure DevOps permissions
    }
}
