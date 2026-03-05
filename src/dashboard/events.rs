use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};

use super::app::DashboardApp;
use crate::orchestrator::Phase;

/// Drain any buffered key events (e.g., Enter from launching the command).
pub fn drain_buffered_events() {
    while event::poll(Duration::from_millis(50)).unwrap_or(false) {
        let _ = event::read();
    }
}

/// Poll for keyboard and mouse events with a timeout.
/// Returns `true` if there was an event to process.
pub fn handle_events(app: &mut DashboardApp, timeout: Duration) -> Result<bool> {
    if event::poll(timeout)? {
        match event::read()? {
            Event::Key(key) => {
                // On Windows, crossterm sends Press, Repeat, and Release events.
                // Only handle Press events to avoid double-processing.
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key);
                }
            }
            Event::Mouse(mouse) if !app.waiting_for_start => {
                handle_mouse(app, mouse);
            }
            _ => {}
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

fn handle_mouse(app: &mut DashboardApp, mouse: MouseEvent) {
    match mouse.kind {
        MouseEventKind::ScrollUp => app.scroll_up_n(3),
        MouseEventKind::ScrollDown => app.scroll_down_n(3),
        _ => {}
    }
}

fn handle_key(app: &mut DashboardApp, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit(),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => app.quit(),
        KeyCode::Char('r') if app.state.finished => app.request_restart(),
        KeyCode::Enter if app.waiting_for_start => app.confirm_start(),
        KeyCode::Enter if app.state.phase == Phase::WaitingForApply => app.confirm_apply(),
        KeyCode::Enter if app.state.phase == Phase::WaitingForMerge => app.confirm_merge(),
        KeyCode::Up | KeyCode::Char('k') if app.waiting_for_start => app.select_prev(),
        KeyCode::Down | KeyCode::Char('j') if app.waiting_for_start => app.select_next(),
        KeyCode::Left | KeyCode::Char('h') if app.waiting_for_start => app.adjust_left(),
        KeyCode::Right | KeyCode::Char('l') if app.waiting_for_start => app.adjust_right(),
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
        _ => {}
    }
}
