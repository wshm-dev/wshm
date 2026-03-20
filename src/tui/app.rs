use anyhow::Result;
use std::collections::HashMap;

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
    Stats,
    Activity,
}

impl Tab {
    pub fn title(&self) -> &'static str {
        match self {
            Tab::Issues => "Issues",
            Tab::PullRequests => "Pull Requests",
            Tab::Queue => "Merge Queue",
            Tab::Stats => "Stats",
            Tab::Activity => "Activity",
        }
    }

    pub fn all() -> &'static [Tab] {
        &[Tab::Issues, Tab::PullRequests, Tab::Queue, Tab::Stats, Tab::Activity]
    }
}

pub struct IssueRow {
    pub issue: Issue,
    pub triage: Option<TriageResultRow>,
}

/// Age bucket for issues/PRs drift analysis.
pub struct AgeBucket {
    pub label: &'static str,
    pub issue_count: usize,
    pub pr_count: usize,
}

pub struct Stats {
    pub by_category: Vec<(String, usize)>,
    pub by_priority: Vec<(String, usize)>,
    pub avg_confidence: f64,
    pub total_triaged: usize,
    pub recent_triages: Vec<TriageResultRow>,
    pub age_buckets: Vec<AgeBucket>,
    pub oldest_issue_days: u64,
    pub oldest_pr_days: u64,
    pub avg_issue_age_days: u64,
    pub avg_pr_age_days: u64,
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
    pub stats: Stats,
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
            stats: Stats {
                by_category: Vec::new(),
                by_priority: Vec::new(),
                avg_confidence: 0.0,
                total_triaged: 0,
                recent_triages: Vec::new(),
                age_buckets: Vec::new(),
                oldest_issue_days: 0,
                oldest_pr_days: 0,
                avg_issue_age_days: 0,
                avg_pr_age_days: 0,
            },
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

        // Build stats from triage results
        self.build_stats();

        self.scroll_offset = 0;
        Ok(())
    }

    fn build_stats(&mut self) {
        let triaged: Vec<&TriageResultRow> = self.issues.iter()
            .filter_map(|r| r.triage.as_ref())
            .collect();

        self.stats.total_triaged = triaged.len();

        // Category breakdown
        let mut cat_map: HashMap<String, usize> = HashMap::new();
        for t in &triaged {
            *cat_map.entry(t.category.clone()).or_default() += 1;
        }
        self.stats.by_category = cat_map.into_iter().collect();
        self.stats.by_category.sort_by(|a, b| b.1.cmp(&a.1));

        // Priority breakdown
        let mut pri_map: HashMap<String, usize> = HashMap::new();
        for t in &triaged {
            let pri = t.priority.clone().unwrap_or_else(|| "unset".to_string());
            *pri_map.entry(pri).or_default() += 1;
        }
        self.stats.by_priority = pri_map.into_iter().collect();
        self.stats.by_priority.sort_by(|a, b| b.1.cmp(&a.1));

        // Average confidence
        if !triaged.is_empty() {
            let sum: f64 = triaged.iter().map(|t| t.confidence).sum();
            self.stats.avg_confidence = sum / triaged.len() as f64;
        }

        // Recent triages (last 20, by acted_at descending)
        let mut recent: Vec<TriageResultRow> = self.issues.iter()
            .filter_map(|r| r.triage.clone())
            .collect();
        recent.sort_by(|a, b| b.acted_at.cmp(&a.acted_at));
        recent.truncate(20);
        self.stats.recent_triages = recent;

        // Age analysis
        let now = chrono::Utc::now();
        let issue_ages: Vec<u64> = self.issues.iter()
            .filter_map(|r| {
                r.issue.created_at.parse::<chrono::DateTime<chrono::Utc>>().ok()
                    .map(|dt| now.signed_duration_since(dt).num_days().max(0) as u64)
            })
            .collect();
        let pr_ages: Vec<u64> = self.pulls.iter()
            .filter_map(|pr| {
                pr.created_at.parse::<chrono::DateTime<chrono::Utc>>().ok()
                    .map(|dt| now.signed_duration_since(dt).num_days().max(0) as u64)
            })
            .collect();

        self.stats.oldest_issue_days = issue_ages.iter().copied().max().unwrap_or(0);
        self.stats.oldest_pr_days = pr_ages.iter().copied().max().unwrap_or(0);
        self.stats.avg_issue_age_days = if issue_ages.is_empty() { 0 } else { issue_ages.iter().sum::<u64>() / issue_ages.len() as u64 };
        self.stats.avg_pr_age_days = if pr_ages.is_empty() { 0 } else { pr_ages.iter().sum::<u64>() / pr_ages.len() as u64 };

        // Age buckets
        let bucket_defs: &[(&str, u64, u64)] = &[
            ("<1d", 0, 1),
            ("1-7d", 1, 7),
            ("7-30d", 7, 30),
            ("30-90d", 30, 90),
            ("90-180d", 90, 180),
            ("180d+", 180, u64::MAX),
        ];
        self.stats.age_buckets = bucket_defs.iter().map(|&(label, min, max)| {
            AgeBucket {
                label,
                issue_count: issue_ages.iter().filter(|&&d| d >= min && d < max).count(),
                pr_count: pr_ages.iter().filter(|&&d| d >= min && d < max).count(),
            }
        }).collect();
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
            Tab::Stats => self.stats.recent_triages.len(),
            Tab::Activity => 0,
        }
    }
}
