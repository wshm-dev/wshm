pub mod app;
pub mod ui;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use std::io;

use crate::config::Config;
use crate::db::Database;

pub async fn run(config: &Config, db: &Database) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = app::App::new(config, db)?;
    let mut last_log_refresh = std::time::Instant::now();

    // Main loop
    let result = run_loop(&mut terminal, &mut app, db, &mut last_log_refresh);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut app::App,
    db: &Database,
    last_log_refresh: &mut std::time::Instant,
) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        // Auto-refresh logs every 5 seconds when on Activity tab
        if app.active_tab == app::Tab::Activity
            && last_log_refresh.elapsed() > std::time::Duration::from_secs(5)
        {
            app.refresh_logs();
            *last_log_refresh = std::time::Instant::now();
        }

        let timeout = std::time::Duration::from_millis(200);
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c')
                    {
                        return Ok(());
                    }
                    // Input mode intercepts all keys
                    if app.input_mode.is_some() {
                        match key.code {
                            KeyCode::Enter => {
                                if app.input_mode == Some(app::InputMode::EditSetting) {
                                    // Apply edited value to settings
                                    if let Some(ref mut settings) = app.settings_popup {
                                        if let Some(item) =
                                            settings.items.get_mut(settings.selected)
                                        {
                                            item.value = app.input_buffer.clone();
                                        }
                                    }
                                    app.input_mode = None;
                                    app.input_buffer.clear();
                                } else {
                                    app.confirm_input();
                                }
                            }
                            KeyCode::Esc => app.cancel_input(),
                            KeyCode::Backspace => {
                                app.input_buffer.pop();
                            }
                            KeyCode::Char(c) => app.input_buffer.push(c),
                            _ => {}
                        }
                        continue;
                    }

                    // Action detail popup
                    if app.action_detail.is_some() {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => {
                                app.close_action_detail()
                            }
                            KeyCode::Up | KeyCode::Char('k') => app.action_detail_scroll_up(),
                            KeyCode::Down | KeyCode::Char('j') => app.action_detail_scroll_down(),
                            _ => {}
                        }
                        continue;
                    }

                    // Settings popup intercepts keys
                    if app.settings_popup.is_some() {
                        match key.code {
                            KeyCode::Up | KeyCode::Char('k') => app.settings_up(),
                            KeyCode::Down | KeyCode::Char('j') => app.settings_down(),
                            KeyCode::Char(' ') | KeyCode::Enter => app.settings_toggle(),
                            KeyCode::Char('e') => app.settings_edit(),
                            KeyCode::Char('s') => app.save_settings(),
                            KeyCode::Esc | KeyCode::Char('q') => app.close_settings(),
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Char('1') => app.active_tab = app::Tab::Repos,
                        KeyCode::Char('2') => {
                            app.load_actions();
                            app.active_tab = app::Tab::Action;
                        }
                        KeyCode::Char('3') => app.active_tab = app::Tab::Issues,
                        KeyCode::Char('4') => app.active_tab = app::Tab::PullRequests,
                        KeyCode::Char('5') => app.active_tab = app::Tab::Queue,
                        KeyCode::Char('6') => app.active_tab = app::Tab::Stats,
                        KeyCode::Char('7') => app.active_tab = app::Tab::Activity,
                        KeyCode::Enter => {
                            if app.active_tab == app::Tab::Repos {
                                app.open_settings();
                            } else if app.active_tab == app::Tab::Action {
                                app.open_action_detail();
                            }
                        }
                        KeyCode::Char(' ') if app.active_tab == app::Tab::Repos => {
                            app.toggle_repo();
                        }
                        KeyCode::Tab => app.next_tab(),
                        KeyCode::BackTab => app.prev_tab(),
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
                        KeyCode::Char('r') => {
                            app.refresh(db)?;
                            app.load_repos();
                            app.load_actions();
                        }
                        KeyCode::Char('n') if app.active_tab == app::Tab::Repos => {
                            app.start_add_repo();
                        }
                        KeyCode::Char('x') if app.active_tab == app::Tab::Repos => {
                            app.start_delete_repo();
                        }
                        KeyCode::Char('s') => app.set_sort(app::SortField::Number),
                        KeyCode::Char('t') => app.set_sort(app::SortField::Title),
                        KeyCode::Char('c') => app.set_sort(app::SortField::Category),
                        KeyCode::Char('p') => app.set_sort(app::SortField::Priority),
                        KeyCode::Char('a') => app.set_sort(app::SortField::Age),
                        KeyCode::Char('o') => app.set_sort(app::SortField::Confidence),
                        KeyCode::Char('m') => app.set_sort(app::SortField::Mergeable),
                        // 'u' — check for update (non-blocking: spawn and poll)
                        KeyCode::Char('u') => {
                            let handle = tokio::runtime::Handle::current();
                            handle.block_on(app.check_update());
                        }
                        // 'U' — apply update (spawned so the TUI isn't blocked)
                        KeyCode::Char('U') => {
                            tokio::spawn(async {
                                let _ = crate::pro_hooks::run_update(true, false).await;
                            });
                        }
                        // 'b' — backup state.db to default path
                        KeyCode::Char('b') => {
                            app.run_backup();
                        }
                        // 'B' — restore: prompt for backup file path
                        KeyCode::Char('B') => {
                            app.start_restore();
                        }
                        _ => {}
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}
