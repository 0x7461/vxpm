mod app;
mod build;
mod dep_graph;
mod git;
mod package;
mod repo;
mod template;
mod ui;
mod version_check;

use std::io;
use std::path::PathBuf;
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
    let void_pkgs = PathBuf::from(format!("{}/void-packages", env!("HOME")));

    if args.len() > 1 && args[1] == "dump" {
        return dump(&void_pkgs);
    }

    run_tui(void_pkgs)
}

fn dump(void_pkgs: &PathBuf) -> Result<()> {
    let names = repo::discover_custom_packages(void_pkgs)?;
    let packages = repo::load_packages(void_pkgs, &names);
    let states = repo::build_package_states(void_pkgs, packages);

    let json = serde_json::to_string_pretty(&states)?;
    println!("{}", json);
    Ok(())
}

fn run_tui(void_pkgs: PathBuf) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = app::App::new(void_pkgs)?;

    loop {
        app.poll_build();
        app.poll_version_check();
        app.poll_git();

        terminal.draw(|f| ui::draw(f, &mut app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // Ctrl+C always quits
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                {
                    break;
                }

                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('j') | KeyCode::Down => app.move_down(),
                    KeyCode::Char('k') | KeyCode::Up => app.move_up(),
                    KeyCode::Enter => app.toggle_detail(),
                    KeyCode::Char('t') => app.toggle_tree(),
                    KeyCode::Char('u') => app.check_versions(),
                    KeyCode::Char('U') => app.bump_and_build(),
                    KeyCode::Char('r') => app.refresh(),
                    KeyCode::Char('b') => app.build_selected(),
                    KeyCode::Char('B') => app.build_with_dependents(),
                    KeyCode::Char('g') => app.open_git_menu(),
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

        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
