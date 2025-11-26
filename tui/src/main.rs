use std::error::Error;
use std::io;
use std::time::{Duration, Instant};

use crossterm::{
    cursor,
    event::{self, Event as CEvent, KeyCode, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

mod app;
mod logo;
mod theme;
mod ui;

use crate::app::create_app_autodetect;

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

    // Automatically select backend: daemon if available, otherwise mock.
    let (mut app, mut backend) = create_app_autodetect();

    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();

    while app.running {
        terminal.draw(|f| ui::draw(f, &app))?;

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
