use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Tabs},
    Frame,
};

use super::app::{self, App, InputMode, SettingKind, SortField, Tab};

fn sort_header(app: &App, label: &str, field: SortField) -> String {
    if app.sort_field == field {
        format!("{} {}", label, app.sort_dir.arrow())
    } else {
        label.to_string()
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header + tabs
            Constraint::Length(3), // Stats bar
            Constraint::Min(10),   // Content
            Constraint::Length(1), // Footer
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_stats(f, app, chunks[1]);

    match app.active_tab {
        Tab::Summary => draw_summary(f, app, chunks[2]),
        Tab::Repos => draw_repos(f, app, chunks[2]),
        Tab::Action => draw_action_plan(f, app, chunks[2]),
        Tab::Issues => draw_issues(f, app, chunks[2]),
        Tab::PullRequests => draw_pulls(f, app, chunks[2]),
        Tab::Queue => draw_queue(f, app, chunks[2]),
        Tab::Stats => draw_stats_tab(f, app, chunks[2]),
        Tab::Activity => draw_activity(f, app, chunks[2]),
    }

    // Action detail popup
    if let Some(ref detail) = app.action_detail {
        draw_action_detail_popup(f, detail);
    }

    // Settings popup overlay
    if let Some(ref settings) = app.settings_popup {
        draw_settings_popup(f, settings, &app.input_mode, &app.input_buffer);
    }

    // Root warning
    if app.is_root {
        let warning = Paragraph::new("⚠ Running as root is not recommended. Use a dedicated user.")
            .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
        f.render_widget(warning, chunks[3]);
    } else {
        draw_footer(f, chunks[3]);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::all()
        .iter()
        .map(|t| Line::from(Span::styled(format!(" {} ", t.title()), Style::default())))
        .collect();

    let idx = Tab::all()
        .iter()
        .position(|t| *t == app.active_tab)
        .unwrap_or(0);

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" wshm - {} ", app.repo_slug)),
        )
        .select(idx)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    f.render_widget(tabs, area);
}

fn draw_stats(f: &mut Frame, app: &App, area: Rect) {
    let stats = vec![
        Span::styled(
            format!(" Issues: {} ", app.open_issue_count),
            Style::default().fg(Color::White),
        ),
        Span::raw(" | "),
        Span::styled(
            format!("Triaged: {} ", app.triaged_count),
            Style::default().fg(Color::Green),
        ),
        Span::raw(" | "),
        Span::styled(
            format!("PRs: {} ", app.open_pr_count),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" | "),
        Span::styled(
            format!(
                "Conflicts: {} ",
                if app.conflict_count > 0 {
                    app.conflict_count.to_string()
                } else {
                    "none".to_string()
                }
            ),
            Style::default().fg(if app.conflict_count > 0 {
                Color::Red
            } else {
                Color::Green
            }),
        ),
    ];

    let para = Paragraph::new(Line::from(stats))
        .block(Block::default().borders(Borders::ALL).title(" Overview "));
    f.render_widget(para, area);
}

fn draw_issues(f: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec![
        sort_header(app, "#(s)", SortField::Number),
        sort_header(app, "Title(t)", SortField::Title),
        sort_header(app, "Cat(c)", SortField::Category),
        sort_header(app, "Conf(o)", SortField::Confidence),
        sort_header(app, "Pri(p)", SortField::Priority),
        sort_header(app, "Age(a)", SortField::Age),
        "PRs".to_string(),
        "Labels".to_string(),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .issues
        .iter()
        .skip(app.scroll_offset)
        .map(|r| {
            let (cat, conf, pri, labels) = if let Some(ref t) = r.triage {
                let cat_style = match t.category.as_str() {
                    "bug" => Style::default().fg(Color::Red),
                    "feature" => Style::default().fg(Color::Cyan),
                    "duplicate" => Style::default().fg(Color::DarkGray),
                    "wontfix" => Style::default().fg(Color::DarkGray),
                    "needs-info" => Style::default().fg(Color::Yellow),
                    _ => Style::default(),
                };
                let pri_style = match t.priority.as_deref() {
                    Some("critical") => {
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                    }
                    Some("high") => Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                    Some("medium") => Style::default().fg(Color::Yellow),
                    Some("low") => Style::default().fg(Color::Green),
                    _ => Style::default(),
                };
                (
                    Cell::from(t.category.clone()).style(cat_style),
                    Cell::from(format!("{:.0}%", t.confidence * 100.0)),
                    Cell::from(t.priority.clone().unwrap_or_else(|| "-".to_string()))
                        .style(pri_style),
                    Cell::from(r.issue.labels.join(", ")),
                )
            } else {
                (
                    Cell::from("-"),
                    Cell::from("-"),
                    Cell::from("-"),
                    Cell::from(r.issue.labels.join(", ")),
                )
            };

            let age_days = r
                .issue
                .created_at
                .parse::<chrono::DateTime<chrono::Utc>>()
                .ok()
                .map(|dt| {
                    chrono::Utc::now()
                        .signed_duration_since(dt)
                        .num_days()
                        .max(0) as u64
                })
                .unwrap_or(0);
            let age_style = if age_days <= 3 {
                Style::default().fg(Color::Green)
            } else if age_days <= 10 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Red)
            };
            let age_text = if age_days == 0 {
                "today".to_string()
            } else {
                format!("{}d", age_days)
            };

            let pr_cell = if r.linked_prs.is_empty() {
                Cell::from("-").style(Style::default().fg(Color::DarkGray))
            } else {
                let pr_text: Vec<String> = r.linked_prs.iter().map(|n| format!("#{n}")).collect();
                Cell::from(pr_text.join(",")).style(Style::default().fg(Color::Green))
            };

            Row::new(vec![
                Cell::from(format!("#{}", r.issue.number)),
                Cell::from(truncate(&r.issue.title, 50)),
                cat,
                conf,
                pri,
                Cell::from(age_text).style(age_style),
                pr_cell,
                labels,
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Min(25),
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Min(12),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Issues ({}) ", app.open_issue_count)),
        )
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");

    f.render_widget(table, area);
}

fn draw_pulls(f: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec![
        sort_header(app, "#(s)", SortField::Number),
        sort_header(app, "Title(t)", SortField::Title),
        "Author".to_string(),
        "Base".to_string(),
        sort_header(app, "Merge(m)", SortField::Mergeable),
        "CI".to_string(),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .pulls
        .iter()
        .skip(app.scroll_offset)
        .map(|pr| {
            let mergeable_style = match pr.mergeable {
                Some(true) => Style::default().fg(Color::Green),
                Some(false) => Style::default().fg(Color::Red),
                None => Style::default().fg(Color::DarkGray),
            };
            let mergeable_text = match pr.mergeable {
                Some(true) => "yes",
                Some(false) => "conflict",
                None => "unknown",
            };

            Row::new(vec![
                Cell::from(format!("#{}", pr.number)),
                Cell::from(truncate(&pr.title, 50)),
                Cell::from(pr.author.clone().unwrap_or_else(|| "-".to_string())),
                Cell::from(pr.base_ref.clone().unwrap_or_else(|| "-".to_string())),
                Cell::from(mergeable_text).style(mergeable_style),
                Cell::from(pr.ci_status.clone().unwrap_or_else(|| "-".to_string())),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Min(30),
        Constraint::Length(15),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Pull Requests ({}) ", app.open_pr_count)),
    );

    f.render_widget(table, area);
}

fn draw_queue(f: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec!["#", "Title", "Mergeable", "CI", "Author"]).style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    // Show only mergeable PRs
    let rows: Vec<Row> = app
        .pulls
        .iter()
        .filter(|pr| pr.mergeable != Some(false))
        .skip(app.scroll_offset)
        .map(|pr| {
            Row::new(vec![
                Cell::from(format!("#{}", pr.number)),
                Cell::from(truncate(&pr.title, 50)),
                Cell::from(if pr.mergeable == Some(true) {
                    "ready"
                } else {
                    "pending"
                })
                .style(if pr.mergeable == Some(true) {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                }),
                Cell::from(pr.ci_status.clone().unwrap_or_else(|| "-".to_string())),
                Cell::from(pr.author.clone().unwrap_or_else(|| "-".to_string())),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Min(30),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(15),
    ];

    let table = Table::new(rows, widths).header(header).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Merge Queue "),
    );

    f.render_widget(table, area);
}

fn draw_stats_tab(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40), // Left: breakdowns
            Constraint::Percentage(60), // Right: recent triages
        ])
        .split(area);

    // Left panel: summary + category + priority + age
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // Summary + age stats
            Constraint::Min(5),     // Category breakdown
            Constraint::Min(5),     // Priority breakdown
            Constraint::Length(10), // Age buckets
        ])
        .split(chunks[0]);

    // Summary + age overview
    let drift_color = |days: u64| -> Color {
        if days > 180 {
            Color::Red
        } else if days > 90 {
            Color::Yellow
        } else {
            Color::Green
        }
    };

    let summary = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                format!(" Triaged: {} ", app.stats.total_triaged),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("Avg confidence: {:.0}%", app.stats.avg_confidence * 100.0),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            Span::raw(" Issues: oldest "),
            Span::styled(
                format!("{}d", app.stats.oldest_issue_days),
                Style::default().fg(drift_color(app.stats.oldest_issue_days)),
            ),
            Span::raw(format!("  avg {}d", app.stats.avg_issue_age_days)),
        ]),
        Line::from(vec![
            Span::raw(" PRs:    oldest "),
            Span::styled(
                format!("{}d", app.stats.oldest_pr_days),
                Style::default().fg(drift_color(app.stats.oldest_pr_days)),
            ),
            Span::raw(format!("  avg {}d", app.stats.avg_pr_age_days)),
        ]),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Summary & Drift "),
    );
    f.render_widget(summary, left_chunks[0]);

    // Category breakdown with bar chart
    let max_cat = app.stats.by_category.first().map(|(_, c)| *c).unwrap_or(1);
    let cat_rows: Vec<Row> = app
        .stats
        .by_category
        .iter()
        .map(|(cat, count)| {
            let bar_len = (*count as f64 / max_cat as f64 * 20.0) as usize;
            let bar = "█".repeat(bar_len);
            let cat_style = match cat.as_str() {
                "bug" => Style::default().fg(Color::Red),
                "feature" => Style::default().fg(Color::Cyan),
                "question" => Style::default().fg(Color::Yellow),
                "docs" => Style::default().fg(Color::Blue),
                _ => Style::default().fg(Color::DarkGray),
            };
            Row::new(vec![
                Cell::from(cat.clone()).style(cat_style),
                Cell::from(count.to_string()),
                Cell::from(bar).style(cat_style),
            ])
        })
        .collect();

    let cat_table = Table::new(
        cat_rows,
        [
            Constraint::Length(12),
            Constraint::Length(5),
            Constraint::Min(10),
        ],
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" By Category "),
    );
    f.render_widget(cat_table, left_chunks[1]);

    // Priority breakdown
    let max_pri = app.stats.by_priority.first().map(|(_, c)| *c).unwrap_or(1);
    let pri_rows: Vec<Row> = app
        .stats
        .by_priority
        .iter()
        .map(|(pri, count)| {
            let bar_len = (*count as f64 / max_pri as f64 * 20.0) as usize;
            let bar = "█".repeat(bar_len);
            let pri_style = match pri.as_str() {
                "critical" => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                "high" => Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                "medium" => Style::default().fg(Color::Yellow),
                "low" => Style::default().fg(Color::Green),
                _ => Style::default().fg(Color::DarkGray),
            };
            Row::new(vec![
                Cell::from(pri.clone()).style(pri_style),
                Cell::from(count.to_string()),
                Cell::from(bar).style(pri_style),
            ])
        })
        .collect();

    let pri_table = Table::new(
        pri_rows,
        [
            Constraint::Length(12),
            Constraint::Length(5),
            Constraint::Min(10),
        ],
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" By Priority "),
    );
    f.render_widget(pri_table, left_chunks[2]);

    // Age buckets
    let max_age_count = app
        .stats
        .age_buckets
        .iter()
        .map(|b| b.issue_count.max(b.pr_count))
        .max()
        .unwrap_or(1)
        .max(1);

    let age_rows: Vec<Row> = app
        .stats
        .age_buckets
        .iter()
        .map(|b| {
            let issue_bar_len = (b.issue_count as f64 / max_age_count as f64 * 12.0) as usize;
            let pr_bar_len = (b.pr_count as f64 / max_age_count as f64 * 12.0) as usize;
            Row::new(vec![
                Cell::from(b.label),
                Cell::from(format!("{:>3}", b.issue_count)),
                Cell::from("█".repeat(issue_bar_len)).style(Style::default().fg(Color::Cyan)),
                Cell::from(format!("{:>3}", b.pr_count)),
                Cell::from("█".repeat(pr_bar_len)).style(Style::default().fg(Color::Magenta)),
            ])
        })
        .collect();

    let age_table = Table::new(
        age_rows,
        [
            Constraint::Length(8),
            Constraint::Length(4),
            Constraint::Length(13),
            Constraint::Length(4),
            Constraint::Length(13),
        ],
    )
    .header(
        Row::new(vec!["Age", "Iss", "", "PRs", ""]).style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Age Distribution "),
    );
    f.render_widget(age_table, left_chunks[3]);

    // Right panel: recent triages
    let header = Row::new(vec!["#", "Category", "Conf", "Priority", "When"]).style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .stats
        .recent_triages
        .iter()
        .skip(app.scroll_offset)
        .map(|t| {
            let cat_style = match t.category.as_str() {
                "bug" => Style::default().fg(Color::Red),
                "feature" => Style::default().fg(Color::Cyan),
                _ => Style::default(),
            };
            let when = if t.acted_at.len() >= 16 {
                &t.acted_at[..16]
            } else {
                &t.acted_at
            };
            Row::new(vec![
                Cell::from(format!("#{}", t.issue_number)),
                Cell::from(t.category.clone()).style(cat_style),
                Cell::from(format!("{:.0}%", t.confidence * 100.0)),
                Cell::from(t.priority.clone().unwrap_or_else(|| "-".to_string())),
                Cell::from(when.to_string()),
            ])
        })
        .collect();

    let recent_table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Min(16),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Recent Triages "),
    );
    f.render_widget(recent_table, chunks[1]);
}

fn draw_action_plan(f: &mut Frame, app: &App, area: Rect) {
    if app.actions.is_empty() {
        let text = Paragraph::new("No action items. All clear!")
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Action Plan "),
            )
            .style(Style::default().fg(Color::Green));
        f.render_widget(text, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from("Pri"),
        Cell::from("Repo"),
        Cell::from("#"),
        Cell::from("Category"),
        Cell::from("Age"),
        Cell::from("PR"),
        Cell::from("Title"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .actions
        .iter()
        .enumerate()
        .skip(app.scroll_offset)
        .map(|(i, item)| {
            let selected = i == app.scroll_offset;
            let (pri_icon, pri_color) = match item.priority.as_str() {
                "critical" => ("🔴", Color::Red),
                "high" => ("🟠", Color::LightRed),
                "medium" => ("🟡", Color::Yellow),
                "low" => ("🟢", Color::Green),
                _ => ("⚪", Color::DarkGray),
            };
            let cat_color = match item.category.as_str() {
                "bug" => Color::Red,
                "feature" => Color::Cyan,
                "question" => Color::Magenta,
                _ => Color::White,
            };
            let age = if item.age_days > 30 {
                format!("{}mo", item.age_days / 30)
            } else {
                format!("{}d", item.age_days)
            };
            let pr = if item.has_pr {
                "✓"
            } else if item.is_simple_fix {
                "⚡"
            } else {
                ""
            };
            let repo_short = item.repo.split('/').next_back().unwrap_or(&item.repo);

            let row_style = if selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(pri_icon).style(Style::default().fg(pri_color)),
                Cell::from(repo_short.to_string()).style(Style::default().fg(Color::DarkGray)),
                Cell::from(format!("#{}", item.issue_number)),
                Cell::from(item.category.clone()).style(Style::default().fg(cat_color)),
                Cell::from(age),
                Cell::from(pr).style(Style::default().fg(Color::Green)),
                Cell::from(item.title.chars().take(60).collect::<String>()),
            ])
            .style(row_style)
        })
        .collect();

    let critical_count = app
        .actions
        .iter()
        .filter(|a| a.priority == "critical")
        .count();
    let high_count = app.actions.iter().filter(|a| a.priority == "high").count();
    let title_color = if critical_count > 0 {
        Color::Red
    } else if high_count > 0 {
        Color::Yellow
    } else {
        Color::Green
    };

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Min(30),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(Span::styled(
        format!(" Action Plan ({}) — 🔴{} 🟠{} 🟡{} ",
                    app.actions.len(), critical_count, high_count,
                    app.actions.iter().filter(|a| a.priority == "medium").count()),
        Style::default().fg(title_color),
    )));

    f.render_widget(table, area);
}

fn draw_repos(f: &mut Frame, app: &App, area: Rect) {
    if app.repos.is_empty() {
        let text =
            Paragraph::new("No repos configured. Edit ~/.wshm/global.toml to add [[repos]].")
                .block(Block::default().borders(Borders::ALL).title(" Repos "))
                .style(Style::default().fg(Color::DarkGray));
        f.render_widget(text, area);
        return;
    }

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from("Repository"),
        Cell::from("Status"),
        Cell::from("Open"),
        Cell::from("Closed"),
        Cell::from("Total"),
        Cell::from("PRs"),
        Cell::from("Triaged"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .repos
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let selected = i == app.scroll_offset;
            let toggle = if r.enabled { "◉" } else { "○" };
            let toggle_color = if r.enabled {
                Color::Green
            } else {
                Color::DarkGray
            };
            let status = if !r.exists {
                ("✗ missing", Color::Red)
            } else if !r.has_wshm {
                ("⚠ no .wshm", Color::Yellow)
            } else {
                ("✓ ready", Color::Green)
            };

            let style = if selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else if !r.enabled {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };

            Row::new(vec![
                Cell::from(toggle).style(Style::default().fg(toggle_color)),
                Cell::from(r.slug.clone()),
                Cell::from(status.0).style(Style::default().fg(status.1)),
                Cell::from(r.open_issues.map(|c| c.to_string()).unwrap_or("–".into()))
                    .style(Style::default().fg(Color::Yellow)),
                Cell::from(r.closed_issues.map(|c| c.to_string()).unwrap_or("–".into()))
                    .style(Style::default().fg(Color::Green)),
                Cell::from(r.total_issues.map(|c| c.to_string()).unwrap_or("–".into())),
                Cell::from(r.open_prs.map(|c| c.to_string()).unwrap_or("–".into()))
                    .style(Style::default().fg(Color::Cyan)),
                Cell::from(r.triaged_count.map(|c| c.to_string()).unwrap_or("–".into())),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(25),
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Length(7),
            Constraint::Length(6),
            Constraint::Length(5),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Repos ({}) ", app.repos.len())),
    );

    // Split area for table + input/help
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);

    f.render_widget(table, layout[0]);

    // Input prompt or help
    if let Some(ref mode) = app.input_mode {
        let (prompt, _hint) = match mode {
            InputMode::AddRepoSlug => (
                "Repo slug (owner/repo): ",
                "Enter to confirm, Esc to cancel",
            ),
            InputMode::AddRepoPath => ("Local path: ", "Enter to confirm, Esc to cancel"),
            InputMode::DeleteConfirm => ("Delete? (y/N): ", ""),
            InputMode::EditSetting => ("New value: ", "Enter to confirm, Esc to cancel"),
        };
        let input = Paragraph::new(Line::from(vec![
            Span::styled(prompt, Style::default().fg(Color::Yellow)),
            Span::raw(&app.input_buffer),
            Span::styled("▌", Style::default().fg(Color::Yellow)),
        ]))
        .block(Block::default().borders(Borders::ALL));
        f.render_widget(input, layout[1]);
    } else {
        let help = Paragraph::new(Span::styled(
            " Space:toggle  Enter:settings  n:add  x:delete  r:refresh  ↑↓:select",
            Style::default().fg(Color::DarkGray),
        ));
        f.render_widget(help, layout[1]);
    }
}

fn draw_action_detail_popup(f: &mut Frame, detail: &app::ActionDetailPopup) {
    let area = f.area();
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(5),
            Constraint::Percentage(90),
            Constraint::Percentage(5),
        ])
        .split(area);
    let popup = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(10),
            Constraint::Percentage(80),
            Constraint::Percentage(10),
        ])
        .split(popup[1]);
    let area = popup[1];

    f.render_widget(ratatui::widgets::Clear, area);

    let item = &detail.item;
    let mut lines: Vec<Line> = Vec::new();

    // Header
    lines.push(Line::from(vec![
        Span::styled(
            format!("#{} ", item.issue_number),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(&item.title, Style::default().add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(""));

    // Meta
    let pri_color = match item.priority.as_str() {
        "critical" => Color::Red,
        "high" => Color::LightRed,
        "medium" => Color::Yellow,
        _ => Color::Green,
    };
    lines.push(Line::from(vec![
        Span::styled("Repo:     ", Style::default().fg(Color::DarkGray)),
        Span::raw(&item.repo),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Priority: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&item.priority, Style::default().fg(pri_color)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Category: ", Style::default().fg(Color::DarkGray)),
        Span::raw(&item.category),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Age:      ", Style::default().fg(Color::DarkGray)),
        Span::raw(format!("{} days", item.age_days)),
    ]));
    if !item.labels.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Labels:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(&item.labels, Style::default().fg(Color::Magenta)),
        ]));
    }
    if item.has_pr {
        lines.push(Line::from(vec![
            Span::styled("PR:       ", Style::default().fg(Color::DarkGray)),
            Span::styled("✓ Has linked PR", Style::default().fg(Color::Green)),
        ]));
    }
    if item.is_simple_fix {
        lines.push(Line::from(vec![
            Span::styled("Fix:      ", Style::default().fg(Color::DarkGray)),
            Span::styled("⚡ Simple fix", Style::default().fg(Color::Yellow)),
        ]));
    }

    // AI Summary
    if !item.summary.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "AI Summary",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for line in item.summary.lines() {
            lines.push(Line::from(format!("  {line}")));
        }
    }

    // Body
    if !item.body.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Description",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for line in item.body.lines().take(30) {
            lines.push(Line::from(format!("  {line}")));
        }
        if item.body.lines().count() > 30 {
            lines.push(Line::from(Span::styled(
                "  ... (truncated)",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    // Comments
    if !item.comments.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Comments ({})", item.comments.len()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for comment in &item.comments {
            let time = if comment.created_at.len() > 16 {
                &comment.created_at[..16]
            } else {
                &comment.created_at
            };
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", comment.author),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(time, Style::default().fg(Color::DarkGray)),
            ]));
            for line in comment.body.lines().take(10) {
                lines.push(Line::from(format!("    {line}")));
            }
            if comment.body.lines().count() > 10 {
                lines.push(Line::from(Span::styled(
                    "    ...",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }

    let paragraph = Paragraph::new(lines)
        .scroll((detail.scroll as u16, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(format!(" {} #{} ", item.repo, item.issue_number))
                .style(Style::default().bg(Color::Black)),
        );

    f.render_widget(paragraph, area);

    // Help
    let help_area = Rect::new(area.x + 1, area.y + area.height - 2, area.width - 2, 1);
    let help = Paragraph::new(Span::styled(
        " ↑↓:scroll  Esc/Enter:close",
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(help, help_area);
}

fn draw_settings_popup(
    f: &mut Frame,
    settings: &app::RepoSettings,
    input_mode: &Option<InputMode>,
    input_buffer: &str,
) {
    let area = f.area();
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(10),
            Constraint::Percentage(80),
            Constraint::Percentage(10),
        ])
        .split(area);
    let popup = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Percentage(70),
            Constraint::Percentage(15),
        ])
        .split(popup[1]);
    let area = popup[1];

    // Clear background
    f.render_widget(ratatui::widgets::Clear, area);

    let mut rows: Vec<ListItem> = Vec::new();
    let mut last_section = String::new();

    for (i, item) in settings.items.iter().enumerate() {
        // Section header
        if item.section != last_section {
            if !last_section.is_empty() {
                rows.push(ListItem::new(Line::from("")));
            }
            rows.push(ListItem::new(Line::from(Span::styled(
                format!("  [{}]", item.section),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))));
            last_section = item.section.clone();
        }

        let selected = i == settings.selected;
        let cursor = if selected { "▸ " } else { "  " };

        let value_style = match item.kind {
            SettingKind::Toggle => {
                if item.value == "true" {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                }
            }
            SettingKind::Text => Style::default().fg(Color::Cyan),
            SettingKind::Label => Style::default().fg(Color::DarkGray),
        };

        let toggle_indicator = match item.kind {
            SettingKind::Toggle => {
                if item.value == "true" {
                    " ◉"
                } else {
                    " ○"
                }
            }
            _ => "",
        };

        let line = Line::from(vec![
            Span::raw(cursor),
            Span::styled(
                &item.key,
                if selected {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ),
            Span::raw(" = "),
            Span::styled(&item.value, value_style),
            Span::styled(toggle_indicator, value_style),
        ]);

        rows.push(ListItem::new(line));
    }

    // Input prompt if editing
    if *input_mode == Some(InputMode::EditSetting) {
        rows.push(ListItem::new(Line::from("")));
        rows.push(ListItem::new(Line::from(vec![
            Span::styled("  New value: ", Style::default().fg(Color::Yellow)),
            Span::raw(input_buffer),
            Span::styled("▌", Style::default().fg(Color::Yellow)),
        ])));
    }

    let list = List::new(rows).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(format!(" {} — Settings ", settings.slug))
            .style(Style::default().bg(Color::Black)),
    );

    f.render_widget(list, area);

    // Help line at bottom of popup
    let help_area = Rect::new(area.x + 1, area.y + area.height - 2, area.width - 2, 1);
    let help = Paragraph::new(Span::styled(
        " Space/Enter:toggle  e:edit  s:save  Esc:close",
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(help, help_area);
}

fn draw_activity(f: &mut Frame, app: &App, area: Rect) {
    if app.logs.is_empty() {
        let text = Paragraph::new("No activity logs. Is the wshm daemon running?")
            .block(Block::default().borders(Borders::ALL).title(" Activity "))
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(text, area);
        return;
    }

    let items: Vec<ListItem> = app
        .logs
        .iter()
        .rev()
        .skip(app.scroll_offset)
        .map(|entry| {
            let (icon, color) = match entry.level.as_str() {
                "ERROR" => ("✗", Color::Red),
                "WARN" => ("⚠", Color::Yellow),
                _ => ("·", Color::White),
            };

            let line = Line::from(vec![
                Span::styled(
                    format!("{} ", entry.timestamp),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(format!("{icon} "), Style::default().fg(color)),
                Span::raw(&entry.message),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Activity ({}) ", app.logs.len())),
    );

    f.render_widget(list, area);
}

fn draw_summary(f: &mut Frame, app: &App, area: Rect) {
    let summary = match app.summary.as_ref() {
        Some(s) => s,
        None => {
            let text = Paragraph::new("No summary data available. Run `wshm sync` first.")
                .block(Block::default().borders(Borders::ALL).title(" Summary "))
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(text, area);
            return;
        }
    };

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        format!("{} — daily digest", summary.repo),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::raw("Issues: "),
        Span::styled(
            summary.open_issues.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" open ({} untriaged)", summary.untriaged_issues)),
    ]));
    lines.push(Line::from(vec![
        Span::raw("PRs:    "),
        Span::styled(
            summary.open_prs.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" open ({} unanalyzed)", summary.unanalyzed_prs)),
    ]));
    if summary.conflicts > 0 {
        lines.push(Line::from(vec![
            Span::raw("Conflicts: "),
            Span::styled(
                summary.conflicts.to_string(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    if !summary.high_priority_issues.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Action Required",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        for i in summary.high_priority_issues.iter().take(10) {
            let prio = i.priority.as_deref().unwrap_or("?");
            let age = if i.age_days > 0 {
                format!(" ({}d)", i.age_days)
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  #{} ", i.number),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(format!("{prio}{age} "), Style::default().fg(Color::Red)),
                Span::raw(i.title.clone()),
            ]));
        }
    }

    if !summary.high_risk_prs.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Attention PRs",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
        for p in summary.high_risk_prs.iter().take(10) {
            let mut tags = Vec::new();
            if let Some(ref risk) = p.risk_level {
                tags.push(format!("risk:{risk}"));
            }
            if p.has_conflicts {
                tags.push("CONFLICT".to_string());
            }
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  #{} ", p.number),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("[{}] ", tags.join(", ")),
                    Style::default().fg(Color::Magenta),
                ),
                Span::raw(p.title.clone()),
            ]));
        }
    }

    if !summary.top_issues.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Issues TODO",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for i in &summary.top_issues {
            let prio = i.priority.as_deref().unwrap_or("-");
            let cat = i.category.as_deref().unwrap_or("-");
            let age = if i.age_days > 0 {
                format!(" ({}d)", i.age_days)
            } else {
                String::new()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  #{} ", i.number),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("{prio}/{cat}{age} "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(i.title.clone()),
            ]));
        }
    }

    if !summary.top_prs.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "PRs TODO",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for p in &summary.top_prs {
            let risk = p.risk_level.as_deref().unwrap_or("-");
            let age = if p.age_days > 0 {
                format!(" ({}d)", p.age_days)
            } else {
                String::new()
            };
            let conflict = if p.has_conflicts { " CONFLICT" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  #{} ", p.number),
                    Style::default().fg(Color::Yellow),
                ),
                Span::styled(
                    format!("{risk}{conflict}{age} "),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(p.title.clone()),
            ]));
        }
    }

    let content = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Summary "))
        .scroll((app.scroll_offset as u16, 0));
    f.render_widget(content, area);
}

fn draw_footer(f: &mut Frame, area: Rect) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" 1-5 ", Style::default().fg(Color::Cyan)),
        Span::raw("tabs  "),
        Span::styled("j/k ", Style::default().fg(Color::Cyan)),
        Span::raw("scroll  "),
        Span::styled("s/t/c/p/o/a/m ", Style::default().fg(Color::Cyan)),
        Span::raw("sort  "),
        Span::styled("r ", Style::default().fg(Color::Cyan)),
        Span::raw("refresh  "),
        Span::styled("q ", Style::default().fg(Color::Cyan)),
        Span::raw("quit"),
    ]));

    f.render_widget(footer, area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let end = s
            .char_indices()
            .nth(max - 1)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}…", &s[..end])
    }
}
