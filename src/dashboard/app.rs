use tokio::sync::{mpsc, oneshot};

use crate::config::{AGENT_PRESETS, StartConfig};
use crate::orchestrator::{OrchestratorCommand, OrchestratorState};

pub struct DashboardApp {
    pub state: OrchestratorState,
    pub scroll_offset: u16,
    pub log_scroll_offset: u16,
    pub should_quit: bool,
    pub waiting_for_start: bool,
    pub restart_requested: bool,
    start_tx: Option<oneshot::Sender<StartConfig>>,
    cmd_tx: mpsc::Sender<OrchestratorCommand>,

    // Interactive start-screen fields
    pub selected_field: usize,
    pub agent_presets: Vec<String>,
    pub agent_a_idx: usize,
    pub agent_b_idx: usize,
    pub edit_rounds: u32,
    pub edit_exchanges: u32,
    pub auto_apply: bool,
    pub apply_level: u32,
    pub language: String,
    pub branch_mode: bool,
}

impl DashboardApp {
    pub fn new(
        initial_state: OrchestratorState,
        start_tx: oneshot::Sender<StartConfig>,
        cmd_tx: mpsc::Sender<OrchestratorCommand>,
    ) -> Self {
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
            restart_requested: false,
            start_tx: Some(start_tx),
            cmd_tx,
            selected_field: 0,
            agent_presets: presets,
            agent_a_idx,
            agent_b_idx,
            edit_rounds,
            edit_exchanges,
            auto_apply: true,
            apply_level: 1,
            language: "en".to_string(),
            branch_mode: false,
        }
    }

    /// Move selection to previous field.
    pub fn select_prev(&mut self) {
        if self.selected_field > 0 {
            self.selected_field -= 1;
        } else {
            self.selected_field = 7;
        }
    }

    /// Move selection to next field.
    pub fn select_next(&mut self) {
        if self.selected_field < 7 {
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
            4 => self.auto_apply = !self.auto_apply,
            5 => self.apply_level = if self.apply_level <= 1 { 3 } else { self.apply_level - 1 },
            6 => self.language = if self.language == "en" { "zh".to_string() } else { "en".to_string() },
            7 => self.branch_mode = !self.branch_mode,
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
            4 => self.auto_apply = !self.auto_apply,
            5 => self.apply_level = if self.apply_level >= 3 { 1 } else { self.apply_level + 1 },
            6 => self.language = if self.language == "en" { "zh".to_string() } else { "en".to_string() },
            7 => self.branch_mode = !self.branch_mode,
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
                auto_apply: self.auto_apply,
                apply_level: self.apply_level,
                language: self.language.clone(),
                branch_mode: self.branch_mode,
            };
            let _ = tx.send(sc);
            self.waiting_for_start = false;
        }
    }

    /// Send ConfirmApply command to orchestrator (blocking).
    pub fn confirm_apply(&self) {
        let _ = self.cmd_tx.blocking_send(OrchestratorCommand::ConfirmApply);
    }

    /// Request a restart (back to start screen after session finishes).
    pub fn request_restart(&mut self) {
        self.restart_requested = true;
        self.should_quit = true;
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
