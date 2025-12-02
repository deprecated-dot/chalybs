use std::error::Error;
use std::io;
use std::time::{Duration, Instant};

use crossterm::{
    cursor,
    event::{self, Event as CEvent, KeyCode, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    Terminal,
};

mod app;
mod config;
mod glitch;
mod logo;
mod logo_png;
mod theme;
mod ui;

use crate::app::create_app_autodetect;
use crate::config::TuiConfig;

/// Recompute the status-panel logo area from the full screen rect.
///
/// This deliberately mirrors the layout in `ui::draw`:
///
///   root
///     ├─ header (3 rows)
///     ├─ body   (middle)
///     │   ├─ status  (left column)
///     │   │   ├─ logo area   (length 7)
///     │   │   └─ VM list
///     │   ├─ events (middle column)
///     │   └─ shell  (right column)
///     └─ footer (1 row)
fn logo_area_from_root(root: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(root);

    let body = vertical[1];

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(26),
            Constraint::Percentage(44),
            Constraint::Percentage(30),
        ])
        .split(body);

    let status_panel = columns[0];

    let splits = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(3)])
        .split(status_panel);

    splits[0]
}

fn main() -> Result<(), Box<dyn Error>> {
    // Run the TUI and ensure the terminal is always restored.
    if let Err(err) = run() {
        cleanup_terminal()?;
        eprintln!("chalybs-tui error: {err}");
        return Err(err);
    }

    cleanup_terminal()?;
    Ok(())
}

fn run() -> Result<(), Box<dyn Error>> {
    // Set up terminal in raw mode + alternate screen.
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Load TUI config (if any) from global config + env.
    let tui_config: Option<TuiConfig> = TuiConfig::load();

    // Automatically select backend: daemon if available, otherwise mock.
    let (mut app, mut backend) = create_app_autodetect(tui_config);

    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    while app.running {
        terminal.draw(|f| ui::draw(f, &app))?;

        // After ratatui has drawn the frame, overlay the PNG logo for
        // capable terminals (kitty etc.). This keeps the ASCII logo as
        // the stable baseline while allowing a richer logo in Kitty.
        {
            let screen_area = terminal.size()?;
            let _logo_area = logo_area_from_root(screen_area);
            // PNG overlay is handled entirely by `logo::draw_logo` /
            // `logo_png::draw_png_logo` inside the UI render path.
        }

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let CEvent::Key(key) = event::read()? {
                // Global key handling with modifier-aware branches.
                match key.code {
                    // Quit: plain 'q'.
                    KeyCode::Char('q') if key.modifiers.is_empty() => {
                        app.running = false;
                    }

                    // Events scroll lock / unlock.
                    KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.lock_events_scroll();
                    }
                    KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.unlock_events_scroll();
                    }

                    // VM detail modal toggle: F2.
                    KeyCode::F(2) => {
                        app.toggle_vm_detail();
                    }

                    // VM selection.
                    KeyCode::Up => {
                        app.select_previous_vm();
                    }
                    KeyCode::Down => {
                        app.select_next_vm();
                    }

                    // VM lifecycle controls (updated to Ctrl+U/O/I).
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.start_selected_vm(&mut backend);
                    }
                    KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.stop_selected_vm(&mut backend);
                    }
                    KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.restart_selected_vm(&mut backend);
                    }

                    // Events scroll via PgUp/PgDn; implies lock.
                    KeyCode::PageUp => {
                        app.lock_events_scroll();
                        app.scroll_events_up(3);
                    }
                    KeyCode::PageDown => {
                        app.lock_events_scroll();
                        app.scroll_events_down(3);
                    }

                    // Backspace edits shell input.
                    KeyCode::Backspace if key.modifiers.is_empty() => {
                        app.pop_shell_char();
                    }

                    // Enter submits shell command.
                    KeyCode::Enter => {
                        app.submit_shell_command(&mut backend);
                    }

                    // Esc: close modal if open, otherwise clear shell input.
                    KeyCode::Esc => {
                        if app.vm_detail_open {
                            app.vm_detail_open = false;
                        } else {
                            app.clear_shell_input();
                        }
                    }

                    // Plain character input goes to shell buffer.
                    KeyCode::Char(c) if key.modifiers.is_empty() => {
                        app.push_shell_char(c);
                    }

                    _ => {}
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            app.on_tick(&mut backend);
            last_tick = Instant::now();
        }
    }

    Ok(())
}

fn cleanup_terminal() -> Result<(), Box<dyn Error>> {
    terminal::disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen, cursor::Show)?;
    Ok(())
}
