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
        Span::styled(" 1-4 ", Style::default().fg(Color::Cyan)),
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
