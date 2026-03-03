use tokio::sync::oneshot;

use crate::config::{AGENT_PRESETS, StartConfig};
use crate::orchestrator::OrchestratorState;

pub struct DashboardApp {
    pub state: OrchestratorState,
    pub scroll_offset: u16,
    pub log_scroll_offset: u16,
    pub should_quit: bool,
    pub waiting_for_start: bool,
    start_tx: Option<oneshot::Sender<StartConfig>>,

    // Interactive start-screen fields
    pub selected_field: usize,
    pub agent_presets: Vec<String>,
    pub agent_a_idx: usize,
    pub agent_b_idx: usize,
    pub edit_rounds: u32,
    pub edit_exchanges: u32,
}

impl DashboardApp {
    pub fn new(initial_state: OrchestratorState, start_tx: oneshot::Sender<StartConfig>) -> Self {
        let presets: Vec<String> = AGENT_PRESETS.iter().map(|s| s.to_string()).collect();

        let agent_a_idx = presets
            .iter()
            .position(|p| p == &initial_state.agent_a_name)
            .unwrap_or(0);
        let agent_b_idx = presets
            .iter()
            .position(|p| p == &initial_state.agent_b_name)
            .unwrap_or(1.min(presets.len().saturating_sub(1)));

        let edit_rounds = initial_state.max_rounds;
        let edit_exchanges = initial_state.max_exchanges;

        Self {
            state: initial_state,
            scroll_offset: 0,
            log_scroll_offset: 0,
            should_quit: false,
            waiting_for_start: true,
            start_tx: Some(start_tx),
            selected_field: 0,
            agent_presets: presets,
            agent_a_idx,
            agent_b_idx,
            edit_rounds,
            edit_exchanges,
        }
    }

    /// Move selection to previous field.
    pub fn select_prev(&mut self) {
        if self.selected_field > 0 {
            self.selected_field -= 1;
        } else {
            self.selected_field = 3;
        }
    }

    /// Move selection to next field.
    pub fn select_next(&mut self) {
        if self.selected_field < 3 {
            self.selected_field += 1;
        } else {
            self.selected_field = 0;
        }
    }

    /// Adjust current field's value to the left (previous preset / decrement).
    pub fn adjust_left(&mut self) {
        let n = self.agent_presets.len();
        match self.selected_field {
            0 => self.agent_a_idx = (self.agent_a_idx + n - 1) % n,
            1 => self.agent_b_idx = (self.agent_b_idx + n - 1) % n,
            2 => self.edit_rounds = (self.edit_rounds.saturating_sub(1)).max(1),
            3 => self.edit_exchanges = (self.edit_exchanges.saturating_sub(1)).max(1),
            _ => {}
        }
    }

    /// Adjust current field's value to the right (next preset / increment).
    pub fn adjust_right(&mut self) {
        let n = self.agent_presets.len();
        match self.selected_field {
            0 => self.agent_a_idx = (self.agent_a_idx + 1) % n,
            1 => self.agent_b_idx = (self.agent_b_idx + 1) % n,
            2 => self.edit_rounds = (self.edit_rounds + 1).min(10),
            3 => self.edit_exchanges = (self.edit_exchanges + 1).min(10),
            _ => {}
        }
    }

    /// User confirmed start — build StartConfig and send it to the orchestrator.
    pub fn confirm_start(&mut self) {
        if let Some(tx) = self.start_tx.take() {
            let sc = StartConfig {
                agent_a_command: self.agent_presets[self.agent_a_idx].clone(),
                agent_b_command: self.agent_presets[self.agent_b_idx].clone(),
                max_rounds: self.edit_rounds,
                max_exchanges: self.edit_exchanges,
            };
            let _ = tx.send(sc);
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
