use anyhow::Result;
use std::collections::HashMap;

use crate::config::{Config, GlobalConfig, RepoEntry};
use crate::db::issues::Issue;
use crate::db::pulls::PullRequest;
use crate::db::triage::TriageResultRow;
use crate::db::Database;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String, // INFO, WARN, ERROR
    pub message: String,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Summary,
    Repos,
    Action,
    Issues,
    PullRequests,
    Queue,
    Stats,
    Activity,
}

impl Tab {
    pub fn title(&self) -> &'static str {
        match self {
            Tab::Summary => "Summary",
            Tab::Repos => "Repos",
            Tab::Action => "Action",
            Tab::Issues => "Issues",
            Tab::PullRequests => "Pull Requests",
            Tab::Queue => "Merge Queue",
            Tab::Stats => "Stats",
            Tab::Activity => "Activity",
        }
    }

    pub fn all() -> &'static [Tab] {
        &[
            Tab::Summary,
            Tab::Repos,
            Tab::Action,
            Tab::Issues,
            Tab::PullRequests,
            Tab::Queue,
            Tab::Stats,
            Tab::Activity,
        ]
    }
}

#[derive(Clone)]
pub struct ActionItem {
    pub repo: String,
    pub repo_path: String,
    pub issue_number: u64,
    pub title: String,
    pub body: String,
    pub category: String,
    pub priority: String,
    pub summary: String,
    pub labels: String,
    pub age_days: i64,
    pub is_simple_fix: bool,
    pub has_pr: bool,
    pub comments: Vec<CommentItem>,
}

#[derive(Clone)]
pub struct CommentItem {
    pub author: String,
    pub body: String,
    pub created_at: String,
}

pub struct ActionDetailPopup {
    pub item: ActionItem,
    pub scroll: usize,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SortField {
    Number,
    Title,
    Category,
    Confidence,
    Priority,
    Age,
    Author,
    Mergeable,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    pub fn toggle(&self) -> Self {
        match self {
            SortDir::Asc => SortDir::Desc,
            SortDir::Desc => SortDir::Asc,
        }
    }

    pub fn arrow(&self) -> &'static str {
        match self {
            SortDir::Asc => "▲",
            SortDir::Desc => "▼",
        }
    }
}

pub struct IssueRow {
    pub issue: Issue,
    pub triage: Option<TriageResultRow>,
    /// PR numbers that reference this issue.
    pub linked_prs: Vec<u64>,
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
    pub sort_field: SortField,
    pub sort_dir: SortDir,

    pub issues: Vec<IssueRow>,
    pub pulls: Vec<PullRequest>,
    pub triaged_count: usize,
    pub open_issue_count: usize,
    pub open_pr_count: usize,
    pub conflict_count: usize,
    pub stats: Stats,
    pub activity: Vec<TriageResultRow>,
    pub logs: Vec<LogEntry>,
    pub actions: Vec<ActionItem>,
    pub repos: Vec<RepoRow>,
    pub global_config_path: Option<PathBuf>,
    pub is_root: bool,
    pub input_mode: Option<InputMode>,
    pub input_buffer: String,
    pub input_step: u8,
    pub input_tmp_slug: String,
    pub confirm_delete: bool,
    pub settings_popup: Option<RepoSettings>,
    pub action_detail: Option<ActionDetailPopup>,
    pub summary: Option<crate::pipelines::status::Summary>,
}

#[derive(Clone, PartialEq)]
pub enum InputMode {
    AddRepoSlug,
    AddRepoPath,
    DeleteConfirm,
    EditSetting,
}

#[derive(Clone)]
pub struct RepoSettings {
    pub slug: String,
    pub path: String,
    pub items: Vec<SettingItem>,
    pub selected: usize,
}

#[derive(Clone)]
pub struct SettingItem {
    pub section: String,
    pub key: String,
    pub value: String,
    pub kind: SettingKind,
}

#[derive(Clone, PartialEq)]
pub enum SettingKind {
    Toggle, // true/false
    Text,   // editable string
    Label,  // read-only info
}

#[derive(Clone)]
pub struct RepoRow {
    pub slug: String,
    pub path: String,
    pub enabled: bool,
    pub exists: bool,
    pub has_wshm: bool,
    pub open_issues: Option<usize>,
    pub closed_issues: Option<usize>,
    pub total_issues: Option<usize>,
    pub open_prs: Option<usize>,
    pub triaged_count: Option<usize>,
}

impl App {
    pub fn new(config: &Config, db: &Database) -> Result<Self> {
        let mut app = Self {
            repo_slug: config.repo_slug(),
            active_tab: Tab::Summary,
            scroll_offset: 0,
            sort_field: SortField::Number,
            sort_dir: SortDir::Desc,
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
            activity: Vec::new(),
            logs: Vec::new(),
            actions: Vec::new(),
            repos: Vec::new(),
            global_config_path: None,
            is_root: std::env::var("USER").unwrap_or_default() == "root",
            input_mode: None,
            input_buffer: String::new(),
            input_step: 0,
            input_tmp_slug: String::new(),
            confirm_delete: false,
            settings_popup: None,
            action_detail: None,
            summary: None,
        };
        app.load_repos();
        app.load_actions();
        app.refresh(db)?;
        app.load_summary(config, db);
        Ok(app)
    }

    pub fn load_summary(&mut self, config: &Config, db: &Database) {
        self.summary = crate::pipelines::status::build_summary(config, db).ok();
    }

    pub fn refresh(&mut self, db: &Database) -> Result<()> {
        let open_issues = db.get_open_issues()?;
        self.open_issue_count = open_issues.len();

        let pulls = db.get_open_pulls()?;

        // Build issue rows with triage results + linked PRs
        self.issues = open_issues
            .into_iter()
            .map(|issue| {
                let triage = db.get_triage_result(issue.number).ok().flatten();
                let issue_ref = format!("#{}", issue.number);
                let linked_prs: Vec<u64> = pulls
                    .iter()
                    .filter(|pr| {
                        pr.body.as_deref().is_some_and(|b| b.contains(&issue_ref))
                            || pr.title.contains(&issue_ref)
                    })
                    .map(|pr| pr.number)
                    .collect();
                IssueRow {
                    issue,
                    triage,
                    linked_prs,
                }
            })
            .collect();

        self.triaged_count = self.issues.iter().filter(|r| r.triage.is_some()).count();
        self.open_pr_count = pulls.len();
        self.conflict_count = pulls.iter().filter(|p| p.mergeable == Some(false)).count();
        self.pulls = pulls;

        // Build stats from triage results
        self.build_stats();

        // Load recent activity
        self.activity = db.recent_activity(50).unwrap_or_default();

        // Load daemon logs
        self.refresh_logs();

        self.scroll_offset = 0;
        Ok(())
    }

    fn build_stats(&mut self) {
        let triaged: Vec<&TriageResultRow> = self
            .issues
            .iter()
            .filter_map(|r| r.triage.as_ref())
            .collect();

        self.stats.total_triaged = triaged.len();

        // Category breakdown
        let mut cat_map: HashMap<String, usize> = HashMap::new();
        for t in &triaged {
            *cat_map.entry(t.category.clone()).or_default() += 1;
        }
        self.stats.by_category = cat_map.into_iter().collect();
        self.stats.by_category.sort_by_key(|b| std::cmp::Reverse(b.1));

        // Priority breakdown
        let mut pri_map: HashMap<String, usize> = HashMap::new();
        for t in &triaged {
            let pri = t.priority.clone().unwrap_or_else(|| "unset".to_string());
            *pri_map.entry(pri).or_default() += 1;
        }
        self.stats.by_priority = pri_map.into_iter().collect();
        self.stats.by_priority.sort_by_key(|b| std::cmp::Reverse(b.1));

        // Average confidence
        if !triaged.is_empty() {
            let sum: f64 = triaged.iter().map(|t| t.confidence).sum();
            self.stats.avg_confidence = sum / triaged.len() as f64;
        }

        // Recent triages (last 20, by acted_at descending)
        let mut recent: Vec<TriageResultRow> = self
            .issues
            .iter()
            .filter_map(|r| r.triage.clone())
            .collect();
        recent.sort_by(|a, b| b.acted_at.cmp(&a.acted_at));
        recent.truncate(20);
        self.stats.recent_triages = recent;

        // Age analysis
        let now = chrono::Utc::now();
        let issue_ages: Vec<u64> = self
            .issues
            .iter()
            .filter_map(|r| {
                r.issue
                    .created_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .ok()
                    .map(|dt| now.signed_duration_since(dt).num_days().max(0) as u64)
            })
            .collect();
        let pr_ages: Vec<u64> = self
            .pulls
            .iter()
            .filter_map(|pr| {
                pr.created_at
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .ok()
                    .map(|dt| now.signed_duration_since(dt).num_days().max(0) as u64)
            })
            .collect();

        self.stats.oldest_issue_days = issue_ages.iter().copied().max().unwrap_or(0);
        self.stats.oldest_pr_days = pr_ages.iter().copied().max().unwrap_or(0);
        self.stats.avg_issue_age_days = if issue_ages.is_empty() {
            0
        } else {
            issue_ages.iter().sum::<u64>() / issue_ages.len() as u64
        };
        self.stats.avg_pr_age_days = if pr_ages.is_empty() {
            0
        } else {
            pr_ages.iter().sum::<u64>() / pr_ages.len() as u64
        };

        // Age buckets
        let bucket_defs: &[(&str, u64, u64)] = &[
            ("<1d", 0, 1),
            ("1-7d", 1, 7),
            ("7-30d", 7, 30),
            ("30-90d", 30, 90),
            ("90-180d", 90, 180),
            ("180d+", 180, u64::MAX),
        ];
        self.stats.age_buckets = bucket_defs
            .iter()
            .map(|&(label, min, max)| AgeBucket {
                label,
                issue_count: issue_ages.iter().filter(|&&d| d >= min && d < max).count(),
                pr_count: pr_ages.iter().filter(|&&d| d >= min && d < max).count(),
            })
            .collect();
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

    /// Set sort field. If same field, toggle direction. Then re-sort.
    pub fn set_sort(&mut self, field: SortField) {
        if self.sort_field == field {
            self.sort_dir = self.sort_dir.toggle();
        } else {
            self.sort_field = field;
            self.sort_dir = SortDir::Desc;
        }
        self.apply_sort();
        self.scroll_offset = 0;
    }

    pub fn apply_sort(&mut self) {
        let dir = self.sort_dir;
        match self.active_tab {
            Tab::Issues => {
                self.issues.sort_by(|a, b| {
                    let cmp = match self.sort_field {
                        SortField::Number => a.issue.number.cmp(&b.issue.number),
                        SortField::Title => a
                            .issue
                            .title
                            .to_lowercase()
                            .cmp(&b.issue.title.to_lowercase()),
                        SortField::Category => {
                            let ac = a.triage.as_ref().map(|t| t.category.as_str()).unwrap_or("");
                            let bc = b.triage.as_ref().map(|t| t.category.as_str()).unwrap_or("");
                            ac.cmp(bc)
                        }
                        SortField::Confidence => {
                            let ac = a.triage.as_ref().map(|t| t.confidence).unwrap_or(0.0);
                            let bc = b.triage.as_ref().map(|t| t.confidence).unwrap_or(0.0);
                            ac.partial_cmp(&bc).unwrap_or(std::cmp::Ordering::Equal)
                        }
                        SortField::Priority => {
                            let pri_rank = |p: Option<&str>| match p {
                                Some("critical") => 0,
                                Some("high") => 1,
                                Some("medium") => 2,
                                Some("low") => 3,
                                _ => 4,
                            };
                            let ap =
                                pri_rank(a.triage.as_ref().and_then(|t| t.priority.as_deref()));
                            let bp =
                                pri_rank(b.triage.as_ref().and_then(|t| t.priority.as_deref()));
                            ap.cmp(&bp)
                        }
                        SortField::Age => a.issue.created_at.cmp(&b.issue.created_at),
                        SortField::Author => {
                            let aa = a.issue.author.as_deref().unwrap_or("");
                            let ba = b.issue.author.as_deref().unwrap_or("");
                            aa.cmp(ba)
                        }
                        _ => std::cmp::Ordering::Equal,
                    };
                    if dir == SortDir::Desc {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                });
            }
            Tab::PullRequests | Tab::Queue => {
                self.pulls.sort_by(|a, b| {
                    let cmp = match self.sort_field {
                        SortField::Number => a.number.cmp(&b.number),
                        SortField::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
                        SortField::Author => {
                            let aa = a.author.as_deref().unwrap_or("");
                            let ba = b.author.as_deref().unwrap_or("");
                            aa.cmp(ba)
                        }
                        SortField::Mergeable => a.mergeable.cmp(&b.mergeable),
                        SortField::Age => a.created_at.cmp(&b.created_at),
                        _ => std::cmp::Ordering::Equal,
                    };
                    if dir == SortDir::Desc {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                });
            }
            _ => {}
        }
    }

    fn current_list_len(&self) -> usize {
        match self.active_tab {
            Tab::Issues => self.issues.len(),
            Tab::PullRequests => self.pulls.len(),
            Tab::Queue => self.pulls.len(),
            Tab::Stats => self.stats.recent_triages.len(),
            Tab::Repos => self.repos.len(),
            Tab::Action => self.actions.len(),
            Tab::Activity => self.logs.len(),
            Tab::Summary => 0,
        }
    }

    /// Load action items across all repos (prioritized TODO list)
    pub fn load_actions(&mut self) {
        let mut items = Vec::new();
        let now = chrono::Utc::now();

        for repo in &self.repos {
            if !repo.enabled || !repo.has_wshm {
                continue;
            }
            let db_path = PathBuf::from(&repo.path).join(".wshm").join("state.db");
            let conn = match rusqlite::Connection::open(&db_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Get open issues with triage results, ordered by priority
            let mut stmt = match conn.prepare(
                "SELECT i.number, i.title, i.created_at, i.body, i.labels,
                        t.category, t.priority, t.is_simple_fix, t.summary
                 FROM issues i
                 LEFT JOIN triage_results t ON i.number = t.issue_number
                 WHERE i.state = 'open'
                 ORDER BY
                    CASE t.priority
                        WHEN 'critical' THEN 0
                        WHEN 'high' THEN 1
                        WHEN 'medium' THEN 2
                        ELSE 3
                    END,
                    i.created_at ASC",
            ) {
                Ok(s) => s,
                Err(_) => continue,
            };

            // Check which issues have linked PRs
            let pr_issues: std::collections::HashSet<u64> = conn
                .prepare("SELECT DISTINCT CAST(SUBSTR(title, INSTR(title, '#') + 1) AS INTEGER) FROM pull_requests WHERE state = 'open' AND title LIKE '%#%'")
                .ok()
                .and_then(|mut s| {
                    s.query_map([], |row| row.get::<_, u64>(0))
                        .ok()
                        .map(|rows| rows.filter_map(|r| r.ok()).collect())
                })
                .unwrap_or_default();

            let repo_path = repo.path.clone();
            let rows = stmt.query_map([], |row| {
                let number: u64 = row.get(0)?;
                let title: String = row.get(1)?;
                let created_at: String = row.get::<_, String>(2).unwrap_or_default();
                let body: String = row.get::<_, String>(3).unwrap_or_default();
                let labels: String = row.get::<_, String>(4).unwrap_or_default();
                let category: String = row
                    .get::<_, String>(5)
                    .unwrap_or_else(|_| "untriaged".into());
                let priority: String = row.get::<_, String>(6).unwrap_or_else(|_| "–".into());
                let is_simple_fix: bool = row.get::<_, bool>(7).unwrap_or(false);
                let summary: String = row.get::<_, String>(8).unwrap_or_default();

                let age_days = chrono::DateTime::parse_from_rfc3339(&created_at)
                    .map(|d| (now - d.with_timezone(&chrono::Utc)).num_days())
                    .unwrap_or(0);

                Ok(ActionItem {
                    repo: String::new(),
                    repo_path: String::new(),
                    issue_number: number,
                    title,
                    body,
                    category,
                    priority,
                    summary,
                    labels,
                    age_days,
                    is_simple_fix,
                    has_pr: false,
                    comments: Vec::new(),
                })
            });

            if let Ok(rows) = rows {
                for row in rows.flatten() {
                    let mut item = row;
                    item.repo = repo.slug.clone();
                    item.repo_path = repo_path.clone();
                    item.has_pr = pr_issues.contains(&item.issue_number);
                    items.push(item);
                }
            }
        }

        // Sort globally by priority
        let priority_order = |p: &str| -> u8 {
            match p {
                "critical" => 0,
                "high" => 1,
                "medium" => 2,
                "low" => 3,
                _ => 4,
            }
        };
        items.sort_by(|a, b| {
            priority_order(&a.priority)
                .cmp(&priority_order(&b.priority))
                .then(b.age_days.cmp(&a.age_days))
        });

        self.actions = items;
    }

    pub fn open_action_detail(&mut self) {
        if let Some(item) = self.actions.get_mut(self.scroll_offset) {
            // Lazy load comments
            if item.comments.is_empty() {
                let db_path = PathBuf::from(&item.repo_path)
                    .join(".wshm")
                    .join("state.db");
                if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                    if let Ok(mut stmt) = conn.prepare(
                        "SELECT author, body, created_at FROM comments WHERE issue_number = ?1 ORDER BY created_at ASC"
                    ) {
                        if let Ok(rows) = stmt.query_map(rusqlite::params![item.issue_number], |row| {
                            Ok(CommentItem {
                                author: row.get::<_, String>(0).unwrap_or_default(),
                                body: row.get::<_, String>(1).unwrap_or_default(),
                                created_at: row.get::<_, String>(2).unwrap_or_default(),
                            })
                        }) {
                            item.comments = rows.flatten().collect();
                        }
                    }
                }
            }

            self.action_detail = Some(ActionDetailPopup {
                item: item.clone(),
                scroll: 0,
            });
        }
    }

    pub fn close_action_detail(&mut self) {
        self.action_detail = None;
    }

    pub fn action_detail_scroll_up(&mut self) {
        if let Some(ref mut d) = self.action_detail {
            d.scroll = d.scroll.saturating_sub(1);
        }
    }

    pub fn action_detail_scroll_down(&mut self) {
        if let Some(ref mut d) = self.action_detail {
            d.scroll += 1;
        }
    }

    /// Load repos from global.toml
    pub fn load_repos(&mut self) {
        let path = GlobalConfig::default_path();
        if !path.exists() {
            return;
        }
        self.global_config_path = Some(path.clone());

        let global = match GlobalConfig::load(&path) {
            Ok(g) => g,
            Err(_) => return,
        };

        self.repos = global
            .repos
            .iter()
            .map(|r| {
                let path_buf = std::path::PathBuf::from(&r.path);
                let exists = path_buf.exists();
                let has_wshm = path_buf.join(".wshm").exists();

                // Try to get counts from the repo's state.db
                let (open_issues, closed_issues, total_issues, open_prs, triaged_count) =
                    if has_wshm {
                        let db_path = path_buf.join(".wshm").join("state.db");
                        if let Ok(conn) = rusqlite::Connection::open(&db_path) {
                            let open: Option<usize> = conn
                                .query_row(
                                    "SELECT COUNT(*) FROM issues WHERE state = 'open'",
                                    [],
                                    |r| r.get(0),
                                )
                                .ok();
                            let closed: Option<usize> = conn
                                .query_row(
                                    "SELECT COUNT(*) FROM issues WHERE state = 'closed'",
                                    [],
                                    |r| r.get(0),
                                )
                                .ok();
                            let total: Option<usize> = conn
                                .query_row("SELECT COUNT(*) FROM issues", [], |r| r.get(0))
                                .ok();
                            let prs: Option<usize> = conn
                                .query_row(
                                    "SELECT COUNT(*) FROM pull_requests WHERE state = 'open'",
                                    [],
                                    |r| r.get(0),
                                )
                                .ok();
                            let triaged: Option<usize> = conn
                                .query_row("SELECT COUNT(*) FROM triage_results", [], |r| r.get(0))
                                .ok();
                            (open, closed, total, prs, triaged)
                        } else {
                            (None, None, None, None, None)
                        }
                    } else {
                        (None, None, None, None, None)
                    };

                RepoRow {
                    slug: r.slug.clone(),
                    path: r.path.to_string_lossy().to_string(),
                    enabled: r.enabled,
                    exists,
                    has_wshm,
                    open_issues,
                    closed_issues,
                    total_issues,
                    open_prs,
                    triaged_count,
                }
            })
            .collect();
    }

    pub fn start_add_repo(&mut self) {
        self.input_mode = Some(InputMode::AddRepoSlug);
        self.input_buffer.clear();
        self.input_tmp_slug.clear();
    }

    pub fn start_delete_repo(&mut self) {
        if !self.repos.is_empty() {
            self.input_mode = Some(InputMode::DeleteConfirm);
        }
    }

    pub fn confirm_input(&mut self) {
        match self.input_mode.clone() {
            Some(InputMode::AddRepoSlug) if !self.input_buffer.is_empty() => {
                self.input_tmp_slug = self.input_buffer.clone();
                self.input_buffer.clear();
                // Default path: ~/slug.split('/').last()
                let default_path = format!(
                    "{}/{}",
                    dirs::home_dir().unwrap_or_default().display(),
                    self.input_tmp_slug.split('/').next_back().unwrap_or("repo")
                );
                self.input_buffer = default_path;
                self.input_mode = Some(InputMode::AddRepoPath);
            }
            Some(InputMode::AddRepoPath) if !self.input_buffer.is_empty() => {
                let path = self.input_buffer.clone();
                self.add_repo(&self.input_tmp_slug.clone(), &path);
                self.input_mode = None;
                self.input_buffer.clear();
            }
            Some(InputMode::DeleteConfirm) => {
                if self.input_buffer.to_lowercase() == "y" {
                    self.delete_selected_repo();
                }
                self.input_mode = None;
                self.input_buffer.clear();
            }
            Some(InputMode::EditSetting) => {
                // Handled in mod.rs directly
            }
            _ => {}
        }
    }

    pub fn cancel_input(&mut self) {
        self.input_mode = None;
        self.input_buffer.clear();
    }

    fn add_repo(&mut self, slug: &str, path: &str) {
        if let Some(ref config_path) = self.global_config_path {
            if let Ok(mut global) = GlobalConfig::load(config_path) {
                global.repos.push(RepoEntry {
                    slug: slug.to_string(),
                    path: PathBuf::from(path),
                    apply: None,
                    enabled: true,
                    secret: None,
                });
                let _ = global.save(config_path);
                self.load_repos();
            }
        }
    }

    fn delete_selected_repo(&mut self) {
        if let Some(repo) = self.repos.get(self.scroll_offset) {
            let slug = repo.slug.clone();
            if let Some(ref config_path) = self.global_config_path {
                if let Ok(mut global) = GlobalConfig::load(config_path) {
                    global.repos.retain(|r| r.slug != slug);
                    let _ = global.save(config_path);
                    self.load_repos();
                    if self.scroll_offset > 0 {
                        self.scroll_offset -= 1;
                    }
                }
            }
        }
    }

    pub fn open_settings(&mut self) {
        if let Some(repo) = self.repos.get(self.scroll_offset) {
            let config_path = PathBuf::from(&repo.path).join(".wshm").join("config.toml");
            let content = std::fs::read_to_string(&config_path).unwrap_or_default();
            let toml_val: toml::Value =
                toml::from_str(&content).unwrap_or(toml::Value::Table(toml::Table::new()));

            let mut items = Vec::new();

            // AI section
            if let Some(ai) = toml_val.get("ai").and_then(|v| v.as_table()) {
                items.push(SettingItem {
                    section: "ai".into(),
                    key: "provider".into(),
                    value: ai
                        .get("provider")
                        .and_then(|v| v.as_str())
                        .unwrap_or("anthropic")
                        .into(),
                    kind: SettingKind::Text,
                });
                items.push(SettingItem {
                    section: "ai".into(),
                    key: "model".into(),
                    value: ai
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("claude-sonnet-4-6")
                        .into(),
                    kind: SettingKind::Text,
                });
            } else {
                items.push(SettingItem {
                    section: "ai".into(),
                    key: "provider".into(),
                    value: "anthropic".into(),
                    kind: SettingKind::Text,
                });
                items.push(SettingItem {
                    section: "ai".into(),
                    key: "model".into(),
                    value: "claude-sonnet-4-6".into(),
                    kind: SettingKind::Text,
                });
            }

            // Triage
            let triage = toml_val.get("triage").and_then(|v| v.as_table());
            items.push(SettingItem {
                section: "triage".into(),
                key: "enabled".into(),
                value: triage
                    .and_then(|t| t.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true)
                    .to_string(),
                kind: SettingKind::Toggle,
            });
            items.push(SettingItem {
                section: "triage".into(),
                key: "auto_fix".into(),
                value: triage
                    .and_then(|t| t.get("auto_fix"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    .to_string(),
                kind: SettingKind::Toggle,
            });

            // PR
            let pr = toml_val.get("pr").and_then(|v| v.as_table());
            items.push(SettingItem {
                section: "pr".into(),
                key: "enabled".into(),
                value: pr
                    .and_then(|t| t.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    .to_string(),
                kind: SettingKind::Toggle,
            });
            items.push(SettingItem {
                section: "pr".into(),
                key: "auto_label".into(),
                value: pr
                    .and_then(|t| t.get("auto_label"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true)
                    .to_string(),
                kind: SettingKind::Toggle,
            });

            // Queue
            let queue = toml_val.get("queue").and_then(|v| v.as_table());
            items.push(SettingItem {
                section: "queue".into(),
                key: "enabled".into(),
                value: queue
                    .and_then(|t| t.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    .to_string(),
                kind: SettingKind::Toggle,
            });

            // Conflicts
            let conflicts = toml_val.get("conflicts").and_then(|v| v.as_table());
            items.push(SettingItem {
                section: "conflicts".into(),
                key: "enabled".into(),
                value: conflicts
                    .and_then(|t| t.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    .to_string(),
                kind: SettingKind::Toggle,
            });

            self.settings_popup = Some(RepoSettings {
                slug: repo.slug.clone(),
                path: repo.path.clone(),
                items,
                selected: 0,
            });
        }
    }

    pub fn settings_up(&mut self) {
        if let Some(ref mut s) = self.settings_popup {
            if s.selected > 0 {
                s.selected -= 1;
            }
        }
    }

    pub fn settings_down(&mut self) {
        if let Some(ref mut s) = self.settings_popup {
            if s.selected < s.items.len().saturating_sub(1) {
                s.selected += 1;
            }
        }
    }

    pub fn settings_toggle(&mut self) {
        if let Some(ref mut settings) = self.settings_popup {
            if let Some(item) = settings.items.get_mut(settings.selected) {
                if item.kind == SettingKind::Toggle {
                    item.value = if item.value == "true" {
                        "false".into()
                    } else {
                        "true".into()
                    };
                }
            }
        }
    }

    pub fn settings_edit(&mut self) {
        if let Some(ref settings) = self.settings_popup {
            if let Some(item) = settings.items.get(settings.selected) {
                if item.kind == SettingKind::Text {
                    self.input_mode = Some(InputMode::EditSetting);
                    self.input_buffer = item.value.clone();
                }
            }
        }
    }

    pub fn save_settings(&mut self) {
        if let Some(ref settings) = self.settings_popup {
            let config_path = PathBuf::from(&settings.path)
                .join(".wshm")
                .join("config.toml");
            let content = std::fs::read_to_string(&config_path).unwrap_or_default();
            let mut toml_val: toml::Value =
                toml::from_str(&content).unwrap_or(toml::Value::Table(toml::Table::new()));

            for item in &settings.items {
                let table = toml_val
                    .as_table_mut()
                    .unwrap()
                    .entry(&item.section)
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                    .as_table_mut()
                    .unwrap();

                let val = match item.kind {
                    SettingKind::Toggle => toml::Value::Boolean(item.value == "true"),
                    SettingKind::Text => toml::Value::String(item.value.clone()),
                    SettingKind::Label => continue,
                };
                table.insert(item.key.clone(), val);
            }

            let _ = std::fs::write(
                &config_path,
                toml::to_string_pretty(&toml_val).unwrap_or_default(),
            );
        }
        self.settings_popup = None;
    }

    pub fn close_settings(&mut self) {
        self.settings_popup = None;
    }

    /// Toggle enabled/disabled for the selected repo and save
    pub fn toggle_repo(&mut self) {
        if let Some(repo) = self.repos.get_mut(self.scroll_offset) {
            repo.enabled = !repo.enabled;

            // Save to global.toml
            if let Some(ref path) = self.global_config_path {
                if let Ok(mut global) = GlobalConfig::load(path) {
                    if let Some(entry) = global.repos.iter_mut().find(|r| r.slug == repo.slug) {
                        entry.enabled = repo.enabled;
                    }
                    let _ = global.save(path);
                }
            }
        }
    }

    /// Load daemon logs from journalctl or build from triage data.
    pub fn refresh_logs(&mut self) {
        // Try journalctl first
        if let Ok(output) = std::process::Command::new("journalctl")
            .args([
                "-u",
                "wshm",
                "--no-pager",
                "-n",
                "100",
                "--output=short-iso",
            ])
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let entries: Vec<LogEntry> = stdout
                    .lines()
                    .filter(|l| !l.starts_with("--"))
                    .filter_map(parse_journal_line)
                    .collect();
                if !entries.is_empty() {
                    self.logs = entries;
                    return;
                }
            }
        }

        // Fallback: build log from triage activity
        self.logs = self
            .activity
            .iter()
            .map(|t| {
                let time = if t.acted_at.len() > 19 {
                    t.acted_at[11..19].to_string()
                } else {
                    t.acted_at.clone()
                };
                LogEntry {
                    timestamp: time,
                    level: "INFO".into(),
                    message: format!(
                        "Triaged #{} → {} ({})",
                        t.issue_number,
                        t.category,
                        t.priority.as_deref().unwrap_or("–")
                    ),
                }
            })
            .collect();
    }
}

/// Parse a journalctl line into a LogEntry.
fn parse_journal_line(line: &str) -> Option<LogEntry> {
    // Format: "2026-03-26T08:21:47+00:00 hostname wshm[pid]: message"
    // or:     "Mar 26 08:21:47 hostname wshm[pid]: message"
    let msg_start = line.find("wshm[")?;
    let after_pid = line[msg_start..].find(']')?;
    let message = line[msg_start + after_pid + 2..].trim().to_string();

    // Extract timestamp
    let timestamp = if line.len() > 19 && line.chars().nth(4) == Some('-') {
        // ISO format
        line[11..19].to_string()
    } else if line.len() > 15 {
        // syslog format "Mar 26 08:21:47"
        line[..15].to_string()
    } else {
        return None;
    };

    // Extract level from tracing output
    let level = if message.contains("ERROR") || message.contains("error") {
        "ERROR"
    } else if message.contains("WARN") || message.contains("warn") {
        "WARN"
    } else {
        "INFO"
    }
    .to_string();

    // Clean tracing prefix (remove ANSI codes and "INFO wshm::module:" prefix)
    let clean_msg = message
        .replace("\x1b[0m", "")
        .replace("\x1b[32m", "")
        .replace("\x1b[33m", "")
        .replace("\x1b[31m", "")
        .replace("\x1b[2m", "")
        .replace("\x1b[1m", "");

    // Remove "INFO wshm::module::name:" prefix
    let clean_msg = if let Some(idx) = clean_msg.find(": ") {
        if clean_msg[..idx].contains("wshm::") {
            clean_msg[idx + 2..].to_string()
        } else {
            clean_msg
        }
    } else {
        clean_msg
    };

    if clean_msg.is_empty() {
        return None;
    }

    Some(LogEntry {
        timestamp,
        level,
        message: clean_msg,
    })
}
