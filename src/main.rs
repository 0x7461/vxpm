mod app;
mod build;
mod config;
mod dep_graph;
mod gcc;
mod git;
mod package;
mod repo;
mod shlibs;
mod template;
mod ui;
mod version_check;

use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cfg = config::load();

    if args.len() > 1 && args[1] == "dump" {
        return dump(&cfg);
    }

    run_tui(cfg)
}

fn dump(cfg: &config::Config) -> Result<()> {
    let names = repo::discover_custom_packages(&cfg.void_packages)?;
    let packages = repo::load_packages(&cfg.void_packages, &names);
    let states = repo::build_package_states(&cfg.void_packages, packages);

    let json = serde_json::to_string_pretty(&states)?;
    println!("{}", json);
    Ok(())
}

fn run_tui(cfg: config::Config) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = app::App::new(cfg.void_packages)?;

    loop {
        app.poll_build();
        app.poll_version_check();
        app.poll_template_bump();
        app.poll_git();

        terminal.draw(|f| ui::draw(f, &mut app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // Ctrl+C always quits
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    break;
                }

                if app.filter_active {
                    match key.code {
                        KeyCode::Esc => app.stop_filter(true),
                        KeyCode::Enter => app.stop_filter(false),
                        KeyCode::Backspace => app.filter_backspace(),
                        KeyCode::Char(c) => app.filter_input(c),
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('/') => app.start_filter(),
                        KeyCode::Char('j') | KeyCode::Down => {
                            if app.panel == app::PanelMode::BuildLog {
                                app.scroll_log_down();
                            } else {
                                app.move_down();
                            }
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            if app.panel == app::PanelMode::BuildLog {
                                app.scroll_log_up();
                            } else {
                                app.move_up();
                            }
                        }
                        KeyCode::Enter => app.toggle_detail(),
                        KeyCode::Tab => app.toggle_tree(),
                        // Upstream check: u = selected, U = all
                        KeyCode::Char('u') => app.check_version_selected(),
                        KeyCode::Char('U') => app.check_versions(),
                        // Template bump (no build): t = selected, T = all
                        KeyCode::Char('t') => app.bump_template_selected(),
                        KeyCode::Char('T') => app.bump_template_all(),
                        // Build (best-effort): b = selected, B = all buildable
                        KeyCode::Char('b') => app.build_selected(),
                        KeyCode::Char('B') => app.build_all_buildable(),
                        // Other
                        KeyCode::Char('r') => app.refresh(),
                        KeyCode::Char('S') if !app.shlib_updates.is_empty() => {
                            app.apply_shlib_updates();
                        }
                        KeyCode::Char('g') => app.open_git_menu(),
                        KeyCode::Char('?') => {
                            app.panel = if app.panel == app::PanelMode::Help {
                                app::PanelMode::None
                            } else {
                                app::PanelMode::Help
                            };
                        }
                        KeyCode::Char('1') if app.panel == app::PanelMode::GitMenu && !app.git_op_active => {
                            app.git_sync_master();
                        }
                        KeyCode::Char('2') if app.panel == app::PanelMode::GitMenu && !app.git_op_active => {
                            app.git_rebase_custom();
                        }
                        KeyCode::Char('3') if app.panel == app::PanelMode::GitMenu && !app.git_op_active => {
                            app.git_push_custom();
                        }
                        KeyCode::Esc => {
                            if app.build_queue.active {
                                app.cancel_build();
                            } else if !app.filter.is_empty() {
                                app.filter.clear();
                                app.selected = 0;
                            } else if app.panel != app::PanelMode::None {
                                app.panel = app::PanelMode::None;
                            } else if app.view == app::View::Tree {
                                app.view = app::View::List;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
