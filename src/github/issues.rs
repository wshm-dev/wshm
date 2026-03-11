use anyhow::{Context, Result};

use crate::db::issues::Issue;
use crate::github::Client;

impl Client {
    pub async fn fetch_issues(&self, since: Option<&str>) -> Result<Vec<Issue>> {
        let issues_handler = self.octocrab.issues(&self.owner, &self.repo);
        let mut builder = issues_handler
            .list()
            .state(octocrab::params::State::Open)
            .per_page(100);

        if let Some(since) = since {
            if let Ok(dt) = since.parse::<chrono::DateTime<chrono::Utc>>() {
                builder = builder.since(dt);
            }
        }

        let mut page = builder.send().await.context("Failed to fetch issues")?;
        let mut all_issues = Vec::new();

        loop {
            for i in page.items {
                if i.pull_request.is_some() {
                    continue; // Exclude PRs from issues endpoint
                }
                all_issues.push(Issue {
                    number: i.number,
                    title: i.title,
                    body: i.body,
                    state: if i.state == octocrab::models::IssueState::Open {
                        "open".to_string()
                    } else {
                        "closed".to_string()
                    },
                    labels: i.labels.iter().map(|l| l.name.clone()).collect(),
                    author: Some(i.user.login),
                    created_at: i.created_at.to_rfc3339(),
                    updated_at: i.updated_at.to_rfc3339(),
                });
            }

            page = match self
                .octocrab
                .get_page::<octocrab::models::issues::Issue>(&page.next)
                .await?
            {
                Some(next) => next,
                None => break,
            };
        }

        Ok(all_issues)
    }

    pub async fn label_issue(&self, number: u64, labels: &[String]) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .add_labels(number, labels)
            .await
            .with_context(|| format!("Failed to label issue #{number}"))?;
        Ok(())
    }

    pub async fn comment_issue(&self, number: u64, body: &str) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .create_comment(number, body)
            .await
            .with_context(|| format!("Failed to comment on issue #{number}"))?;
        Ok(())
    }

    pub async fn close_issue(&self, number: u64) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .update(number)
            .state(octocrab::models::IssueState::Closed)
            .send()
            .await
            .with_context(|| format!("Failed to close issue #{number}"))?;
        Ok(())
    }
}
