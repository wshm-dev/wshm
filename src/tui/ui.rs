use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs},
    Frame,
};

use super::app::{App, Tab};

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header + tabs
            Constraint::Length(3), // Stats bar
            Constraint::Min(10),  // Content
            Constraint::Length(1), // Footer
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_stats(f, app, chunks[1]);

    match app.active_tab {
        Tab::Issues => draw_issues(f, app, chunks[2]),
        Tab::PullRequests => draw_pulls(f, app, chunks[2]),
        Tab::Queue => draw_queue(f, app, chunks[2]),
        Tab::Stats => draw_stats_tab(f, app, chunks[2]),
        Tab::Activity => draw_activity(f, app, chunks[2]),
    }

    draw_footer(f, chunks[3]);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::all()
        .iter()
        .map(|t| Line::from(format!(" {} ", t.title())))
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

    let para = Paragraph::new(Line::from(stats)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Overview "),
    );
    f.render_widget(para, area);
}

fn draw_issues(f: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec!["#", "Title", "Category", "Conf", "Priority", "Labels"])
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
                    Some("critical") => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    Some("high") => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    Some("medium") => Style::default().fg(Color::Yellow),
                    Some("low") => Style::default().fg(Color::Green),
                    _ => Style::default(),
                };
                (
                    Cell::from(t.category.clone()).style(cat_style),
                    Cell::from(format!("{:.0}%", t.confidence * 100.0)),
                    Cell::from(t.priority.clone().unwrap_or_else(|| "-".to_string())).style(pri_style),
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

            Row::new(vec![
                Cell::from(format!("#{}", r.issue.number)),
                Cell::from(truncate(&r.issue.title, 50)),
                cat,
                conf,
                pri,
                labels,
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Min(30),
        Constraint::Length(12),
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Min(20),
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
    let header = Row::new(vec!["#", "Title", "Author", "Base", "Mergeable", "CI"])
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

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Pull Requests ({}) ", app.open_pr_count)),
        );

    f.render_widget(table, area);
}

fn draw_queue(f: &mut Frame, app: &App, area: Rect) {
    let header = Row::new(vec!["#", "Title", "Mergeable", "CI", "Author"])
        .style(
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
                Cell::from(if pr.mergeable == Some(true) { "ready" } else { "pending" })
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

    let table = Table::new(rows, widths)
        .header(header)
        .block(
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
            Constraint::Min(5),    // Category breakdown
            Constraint::Min(5),    // Priority breakdown
            Constraint::Length(10), // Age buckets
        ])
        .split(chunks[0]);

    // Summary + age overview
    let drift_color = |days: u64| -> Color {
        if days > 180 { Color::Red }
        else if days > 90 { Color::Yellow }
        else { Color::Green }
    };

    let summary = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(format!(" Triaged: {} ", app.stats.total_triaged), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("Avg confidence: {:.0}%", app.stats.avg_confidence * 100.0), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw(" Issues: oldest "),
            Span::styled(format!("{}d", app.stats.oldest_issue_days), Style::default().fg(drift_color(app.stats.oldest_issue_days))),
            Span::raw(format!("  avg {}d", app.stats.avg_issue_age_days)),
        ]),
        Line::from(vec![
            Span::raw(" PRs:    oldest "),
            Span::styled(format!("{}d", app.stats.oldest_pr_days), Style::default().fg(drift_color(app.stats.oldest_pr_days))),
            Span::raw(format!("  avg {}d", app.stats.avg_pr_age_days)),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL).title(" Summary & Drift "));
    f.render_widget(summary, left_chunks[0]);

    // Category breakdown with bar chart
    let max_cat = app.stats.by_category.first().map(|(_, c)| *c).unwrap_or(1);
    let cat_rows: Vec<Row> = app.stats.by_category.iter().map(|(cat, count)| {
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
    }).collect();

    let cat_table = Table::new(
        cat_rows,
        [Constraint::Length(12), Constraint::Length(5), Constraint::Min(10)],
    )
    .block(Block::default().borders(Borders::ALL).title(" By Category "));
    f.render_widget(cat_table, left_chunks[1]);

    // Priority breakdown
    let max_pri = app.stats.by_priority.first().map(|(_, c)| *c).unwrap_or(1);
    let pri_rows: Vec<Row> = app.stats.by_priority.iter().map(|(pri, count)| {
        let bar_len = (*count as f64 / max_pri as f64 * 20.0) as usize;
        let bar = "█".repeat(bar_len);
        let pri_style = match pri.as_str() {
            "critical" => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            "high" => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            "medium" => Style::default().fg(Color::Yellow),
            "low" => Style::default().fg(Color::Green),
            _ => Style::default().fg(Color::DarkGray),
        };
        Row::new(vec![
            Cell::from(pri.clone()).style(pri_style),
            Cell::from(count.to_string()),
            Cell::from(bar).style(pri_style),
        ])
    }).collect();

    let pri_table = Table::new(
        pri_rows,
        [Constraint::Length(12), Constraint::Length(5), Constraint::Min(10)],
    )
    .block(Block::default().borders(Borders::ALL).title(" By Priority "));
    f.render_widget(pri_table, left_chunks[2]);

    // Age buckets
    let max_age_count = app.stats.age_buckets.iter()
        .map(|b| b.issue_count.max(b.pr_count))
        .max()
        .unwrap_or(1)
        .max(1);

    let age_rows: Vec<Row> = app.stats.age_buckets.iter().map(|b| {
        let issue_bar_len = (b.issue_count as f64 / max_age_count as f64 * 12.0) as usize;
        let pr_bar_len = (b.pr_count as f64 / max_age_count as f64 * 12.0) as usize;
        Row::new(vec![
            Cell::from(b.label),
            Cell::from(format!("{:>3}", b.issue_count)),
            Cell::from("█".repeat(issue_bar_len)).style(Style::default().fg(Color::Cyan)),
            Cell::from(format!("{:>3}", b.pr_count)),
            Cell::from("█".repeat(pr_bar_len)).style(Style::default().fg(Color::Magenta)),
        ])
    }).collect();

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
        Row::new(vec!["Age", "Iss", "", "PRs", ""])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(" Age Distribution "));
    f.render_widget(age_table, left_chunks[3]);

    // Right panel: recent triages
    let header = Row::new(vec!["#", "Category", "Conf", "Priority", "When"])
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = app.stats.recent_triages.iter().skip(app.scroll_offset).map(|t| {
        let cat_style = match t.category.as_str() {
            "bug" => Style::default().fg(Color::Red),
            "feature" => Style::default().fg(Color::Cyan),
            _ => Style::default(),
        };
        let when = if t.acted_at.len() >= 16 { &t.acted_at[..16] } else { &t.acted_at };
        Row::new(vec![
            Cell::from(format!("#{}", t.issue_number)),
            Cell::from(t.category.clone()).style(cat_style),
            Cell::from(format!("{:.0}%", t.confidence * 100.0)),
            Cell::from(t.priority.clone().unwrap_or_else(|| "-".to_string())),
            Cell::from(when.to_string()),
        ])
    }).collect();

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
    .block(Block::default().borders(Borders::ALL).title(" Recent Triages "));
    f.render_widget(recent_table, chunks[1]);
}

fn draw_activity(f: &mut Frame, _app: &App, area: Rect) {
    let text = Paragraph::new("Activity log coming soon. Press 'r' to refresh data.")
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Activity "),
        )
        .style(Style::default().fg(Color::DarkGray));

    f.render_widget(text, area);
}

fn draw_footer(f: &mut Frame, area: Rect) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" 1-5 ", Style::default().fg(Color::Cyan)),
        Span::raw("tabs  "),
        Span::styled("j/k ", Style::default().fg(Color::Cyan)),
        Span::raw("scroll  "),
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
        let end = s.char_indices().nth(max - 1).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}…", &s[..end])
    }
}
