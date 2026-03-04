use tokio::sync::{mpsc, oneshot};

use super::LOG_VISIBLE_LINES;
use crate::config::{AGENT_PRESETS, StartConfig};
use crate::orchestrator::{OrchestratorCommand, OrchestratorState};

const TIMEOUT_MIN_SECS: u64 = 60;
const TIMEOUT_MAX_SECS: u64 = 3600;
const TIMEOUT_STEP_SECS: u64 = 60;
const START_FIELD_COUNT: usize = 10;
const START_LAST_FIELD: usize = START_FIELD_COUNT - 1;

pub struct DashboardApp {
    pub state: OrchestratorState,
    pub scroll_offset: u16,
    pub log_scroll_offset: u16,
    pub conversation_visible_lines: u16,
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
    pub edit_agent_a_timeout_secs: u64,
    pub edit_agent_b_timeout_secs: u64,
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
        let edit_agent_a_timeout_secs = initial_state
            .agent_a_timeout_secs
            .clamp(TIMEOUT_MIN_SECS, TIMEOUT_MAX_SECS);
        let edit_agent_b_timeout_secs = initial_state
            .agent_b_timeout_secs
            .clamp(TIMEOUT_MIN_SECS, TIMEOUT_MAX_SECS);

        Self {
            state: initial_state,
            scroll_offset: 0,
            log_scroll_offset: 0,
            conversation_visible_lines: 1,
            should_quit: false,
            waiting_for_start: true,
            restart_requested: false,
            start_tx: Some(start_tx),
            cmd_tx,
            selected_field: 0,
            agent_presets: presets,
            agent_a_idx,
            agent_b_idx,
            edit_agent_a_timeout_secs,
            edit_agent_b_timeout_secs,
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
            self.selected_field = START_LAST_FIELD;
        }
    }

    /// Move selection to next field.
    pub fn select_next(&mut self) {
        if self.selected_field < START_LAST_FIELD {
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
            1 => {
                self.edit_agent_a_timeout_secs = self
                    .edit_agent_a_timeout_secs
                    .saturating_sub(TIMEOUT_STEP_SECS)
                    .max(TIMEOUT_MIN_SECS)
            }
            2 => self.agent_b_idx = (self.agent_b_idx + n - 1) % n,
            3 => {
                self.edit_agent_b_timeout_secs = self
                    .edit_agent_b_timeout_secs
                    .saturating_sub(TIMEOUT_STEP_SECS)
                    .max(TIMEOUT_MIN_SECS)
            }
            4 => self.edit_rounds = (self.edit_rounds.saturating_sub(1)).max(1),
            5 => self.edit_exchanges = (self.edit_exchanges.saturating_sub(1)).max(1),
            6 => self.auto_apply = !self.auto_apply,
            7 => {
                self.apply_level = if self.apply_level <= 1 {
                    3
                } else {
                    self.apply_level - 1
                }
            }
            8 => {
                self.language = if self.language == "en" {
                    "zh".to_string()
                } else {
                    "en".to_string()
                }
            }
            9 => self.branch_mode = !self.branch_mode,
            _ => {}
        }
    }

    /// Adjust current field's value to the right (next preset / increment).
    pub fn adjust_right(&mut self) {
        let n = self.agent_presets.len();
        match self.selected_field {
            0 => self.agent_a_idx = (self.agent_a_idx + 1) % n,
            1 => {
                self.edit_agent_a_timeout_secs =
                    (self.edit_agent_a_timeout_secs + TIMEOUT_STEP_SECS).min(TIMEOUT_MAX_SECS)
            }
            2 => self.agent_b_idx = (self.agent_b_idx + 1) % n,
            3 => {
                self.edit_agent_b_timeout_secs =
                    (self.edit_agent_b_timeout_secs + TIMEOUT_STEP_SECS).min(TIMEOUT_MAX_SECS)
            }
            4 => self.edit_rounds = (self.edit_rounds + 1).min(10),
            5 => self.edit_exchanges = (self.edit_exchanges + 1).min(10),
            6 => self.auto_apply = !self.auto_apply,
            7 => {
                self.apply_level = if self.apply_level >= 3 {
                    1
                } else {
                    self.apply_level + 1
                }
            }
            8 => {
                self.language = if self.language == "en" {
                    "zh".to_string()
                } else {
                    "en".to_string()
                }
            }
            9 => self.branch_mode = !self.branch_mode,
            _ => {}
        }
    }

    /// User confirmed start — build StartConfig and send it to the orchestrator.
    pub fn confirm_start(&mut self) {
        if let Some(tx) = self.start_tx.take() {
            let sc = StartConfig {
                agent_a_command: self.agent_presets[self.agent_a_idx].clone(),
                agent_b_command: self.agent_presets[self.agent_b_idx].clone(),
                agent_a_timeout_secs: self.edit_agent_a_timeout_secs,
                agent_b_timeout_secs: self.edit_agent_b_timeout_secs,
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

    /// Send ConfirmMerge command to orchestrator (blocking).
    pub fn confirm_merge(&self) {
        let _ = self.cmd_tx.blocking_send(OrchestratorCommand::ConfirmMerge);
    }

    /// Request a restart (back to start screen after session finishes).
    pub fn request_restart(&mut self) {
        self.restart_requested = true;
        self.should_quit = true;
    }

    pub fn update_state(&mut self, new_state: OrchestratorState) {
        self.state = new_state;
        self.language = self.state.language.clone();
        // Auto-scroll logs to bottom.
        let log_len = self.state.logs.len() as u16;
        if log_len > LOG_VISIBLE_LINES {
            self.log_scroll_offset = log_len - LOG_VISIBLE_LINES;
        }
        self.scroll_offset = self
            .scroll_offset
            .min(self.max_conversation_scroll_offset());
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn set_conversation_visible_lines(&mut self, visible_lines: u16) {
        self.conversation_visible_lines = visible_lines.max(1);
        self.scroll_offset = self
            .scroll_offset
            .min(self.max_conversation_scroll_offset());
    }

    fn max_conversation_scroll_offset(&self) -> u16 {
        let total_lines = self.state.conversation_preview.lines().count() as u16;
        total_lines.saturating_sub(self.conversation_visible_lines)
    }

    pub fn scroll_down(&mut self) {
        if self.scroll_offset < self.max_conversation_scroll_offset() {
            self.scroll_offset = self.scroll_offset.saturating_add(1);
        }
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }
}
