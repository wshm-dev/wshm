use anyhow::{Context, Result};

use crate::db::pulls::PullRequest;
use crate::github::Client;

impl Client {
    pub async fn fetch_pulls(&self) -> Result<Vec<PullRequest>> {
        let mut page = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .list()
            .state(octocrab::params::State::Open)
            .per_page(100)
            .send()
            .await
            .context("Failed to fetch pull requests")?;

        let mut all_pulls = Vec::new();

        loop {
            for pr in page.items {
                all_pulls.push(PullRequest {
                    number: pr.number,
                    title: pr.title.unwrap_or_default(),
                    body: pr.body,
                    state: if pr.state == Some(octocrab::models::IssueState::Open) {
                        "open".to_string()
                    } else {
                        "closed".to_string()
                    },
                    labels: pr
                        .labels
                        .unwrap_or_default()
                        .iter()
                        .map(|l| l.name.clone())
                        .collect(),
                    author: pr.user.map(|u| u.login),
                    head_sha: Some(pr.head.sha),
                    base_sha: Some(pr.base.sha),
                    head_ref: Some(pr.head.ref_field),
                    base_ref: Some(pr.base.ref_field),
                    mergeable: pr.mergeable,
                    ci_status: None,
                    created_at: pr.created_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
                    updated_at: pr.updated_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
                });
            }

            page = match self
                .octocrab
                .get_page::<octocrab::models::pulls::PullRequest>(&page.next)
                .await?
            {
                Some(next) => next,
                None => break,
            };
        }

        Ok(all_pulls)
    }

    pub async fn fetch_pr_diff(&self, number: u64) -> Result<String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{number}",
            self.owner, self.repo
        );

        let response = self
            .octocrab
            ._get(&url)
            .await
            .with_context(|| format!("Failed to fetch diff for PR #{number}"))?;

        let text = self
            .octocrab
            .body_to_string(response)
            .await
            .with_context(|| format!("Failed to read diff body for PR #{number}"))?;

        Ok(text)
    }

    pub async fn label_pr(&self, number: u64, labels: &[String]) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .add_labels(number, labels)
            .await
            .with_context(|| format!("Failed to label PR #{number}"))?;
        Ok(())
    }

    pub async fn comment_pr(&self, number: u64, body: &str) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .create_comment(number, body)
            .await
            .with_context(|| format!("Failed to comment on PR #{number}"))?;
        Ok(())
    }
}
