use anyhow::Result;

use crate::config::Config;
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;
use crate::db::triage::TriageResultRow;
use crate::db::Database;

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Issues,
    PullRequests,
    Queue,
    Activity,
}

impl Tab {
    pub fn title(&self) -> &'static str {
        match self {
            Tab::Issues => "Issues",
            Tab::PullRequests => "Pull Requests",
            Tab::Queue => "Merge Queue",
            Tab::Activity => "Activity",
        }
    }

    pub fn all() -> &'static [Tab] {
        &[Tab::Issues, Tab::PullRequests, Tab::Queue, Tab::Activity]
    }
}

pub struct IssueRow {
    pub issue: Issue,
    pub triage: Option<TriageResultRow>,
}

pub struct App {
    pub repo_slug: String,
    pub active_tab: Tab,
    pub scroll_offset: usize,

    pub issues: Vec<IssueRow>,
    pub pulls: Vec<PullRequest>,
    pub triaged_count: usize,
    pub open_issue_count: usize,
    pub open_pr_count: usize,
    pub conflict_count: usize,
}

impl App {
    pub fn new(config: &Config, db: &Database) -> Result<Self> {
        let mut app = Self {
            repo_slug: config.repo_slug(),
            active_tab: Tab::Issues,
            scroll_offset: 0,
            issues: Vec::new(),
            pulls: Vec::new(),
            triaged_count: 0,
            open_issue_count: 0,
            open_pr_count: 0,
            conflict_count: 0,
        };
        app.refresh(db)?;
        Ok(app)
    }

    pub fn refresh(&mut self, db: &Database) -> Result<()> {
        let open_issues = db.get_open_issues()?;
        self.open_issue_count = open_issues.len();

        // Build issue rows with triage results
        self.issues = open_issues
            .into_iter()
            .map(|issue| {
                let triage = db.get_triage_result(issue.number).ok().flatten();
                IssueRow { issue, triage }
            })
            .collect();

        self.triaged_count = self.issues.iter().filter(|r| r.triage.is_some()).count();

        let pulls = db.get_open_pulls()?;
        self.open_pr_count = pulls.len();
        self.conflict_count = pulls.iter().filter(|p| p.mergeable == Some(false)).count();
        self.pulls = pulls;

        self.scroll_offset = 0;
        Ok(())
    }

    pub fn next_tab(&mut self) {
        let tabs = Tab::all();
        let idx = tabs.iter().position(|t| *t == self.active_tab).unwrap_or(0);
        self.active_tab = tabs[(idx + 1) % tabs.len()];
        self.scroll_offset = 0;
    }

    pub fn prev_tab(&mut self) {
        let tabs = Tab::all();
        let idx = tabs.iter().position(|t| *t == self.active_tab).unwrap_or(0);
        self.active_tab = tabs[(idx + tabs.len() - 1) % tabs.len()];
        self.scroll_offset = 0;
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        let max = self.current_list_len().saturating_sub(1);
        if self.scroll_offset < max {
            self.scroll_offset += 1;
        }
    }

    fn current_list_len(&self) -> usize {
        match self.active_tab {
            Tab::Issues => self.issues.len(),
            Tab::PullRequests => self.pulls.len(),
            Tab::Queue => self.pulls.len(),
            Tab::Activity => 0,
        }
    }
}
