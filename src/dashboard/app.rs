use tokio::sync::oneshot;

use crate::orchestrator::OrchestratorState;

pub struct DashboardApp {
    pub state: OrchestratorState,
    pub scroll_offset: u16,
    pub log_scroll_offset: u16,
    pub should_quit: bool,
    pub waiting_for_start: bool,
    start_tx: Option<oneshot::Sender<()>>,
}

impl DashboardApp {
    pub fn new(initial_state: OrchestratorState, start_tx: oneshot::Sender<()>) -> Self {
        Self {
            state: initial_state,
            scroll_offset: 0,
            log_scroll_offset: 0,
            should_quit: false,
            waiting_for_start: true,
            start_tx: Some(start_tx),
        }
    }

    /// User confirmed start — send the signal to the orchestrator.
    pub fn confirm_start(&mut self) {
        if let Some(tx) = self.start_tx.take() {
            let _ = tx.send(());
            self.waiting_for_start = false;
        }
    }

    pub fn update_state(&mut self, new_state: OrchestratorState) {
        self.state = new_state;
        // Auto-scroll logs to bottom (log panel is Length(6) - 2 borders = 4 visible lines)
        let log_len = self.state.logs.len() as u16;
        if log_len > 4 {
            self.log_scroll_offset = log_len - 4;
        }
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        let total_lines = self.state.conversation_preview.lines().count() as u16;
        if self.scroll_offset < total_lines.saturating_sub(1) {
            self.scroll_offset = self.scroll_offset.saturating_add(1);
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }
}
