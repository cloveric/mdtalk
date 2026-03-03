use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::app::DashboardApp;

/// Drain any buffered key events (e.g., Enter from launching the command).
pub fn drain_buffered_events() {
    while event::poll(Duration::from_millis(50)).unwrap_or(false) {
        let _ = event::read();
    }
}

/// Poll for keyboard events with a timeout.
/// Returns `true` if there was an event to process.
pub fn handle_events(app: &mut DashboardApp, timeout: Duration) -> Result<bool> {
    if event::poll(timeout)? {
        if let Event::Key(key) = event::read()? {
            // On Windows, crossterm sends Press, Repeat, and Release events.
            // Only handle Press events to avoid double-processing.
            if key.kind == KeyEventKind::Press {
                handle_key(app, key);
            }
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

fn handle_key(app: &mut DashboardApp, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit(),
        KeyCode::Enter if app.waiting_for_start => app.confirm_start(),
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
        _ => {}
    }
}
