use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct MdtalkConfig {
    pub project: ProjectConfig,
    #[serde(default = "default_agent_a")]
    pub agent_a: AgentConfig,
    #[serde(default = "default_agent_b")]
    pub agent_b: AgentConfig,
    #[serde(default)]
    pub review: ReviewConfig,
    #[serde(default)]
    #[allow(dead_code)]
    pub dashboard: DashboardConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub command: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewConfig {
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u32,
    #[serde(default = "default_max_exchanges")]
    pub max_exchanges: u32,
    #[serde(default = "default_consensus_keywords")]
    pub consensus_keywords: Vec<String>,
    #[serde(default = "default_output_file")]
    pub output_file: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct DashboardConfig {
    #[serde(default = "default_refresh_rate")]
    pub refresh_rate_ms: u64,
}

fn default_agent_a() -> AgentConfig {
    AgentConfig {
        name: "claude".to_string(),
        command: "claude".to_string(),
        timeout_secs: default_timeout(),
    }
}

fn default_agent_b() -> AgentConfig {
    AgentConfig {
        name: "codex".to_string(),
        command: "codex".to_string(),
        timeout_secs: default_timeout(),
    }
}

fn default_timeout() -> u64 {
    600
}
fn default_max_rounds() -> u32 {
    1
}
fn default_max_exchanges() -> u32 {
    5
}
fn default_consensus_keywords() -> Vec<String> {
    vec![
        "agree".to_string(),
        "consensus".to_string(),
        "达成一致".to_string(),
        "同意".to_string(),
        "no further".to_string(),
        "looks good".to_string(),
        "LGTM".to_string(),
    ]
}
fn default_output_file() -> String {
    "conversation.md".to_string()
}
fn default_refresh_rate() -> u64 {
    500
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            max_rounds: default_max_rounds(),
            max_exchanges: default_max_exchanges(),
            consensus_keywords: default_consensus_keywords(),
            output_file: default_output_file(),
        }
    }
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            refresh_rate_ms: default_refresh_rate(),
        }
    }
}

/// Agent command presets available in the start screen.
pub const AGENT_PRESETS: &[&str] = &["claude", "codex", "gemini"];

/// Settings chosen by the user on the interactive start screen.
#[derive(Debug, Clone)]
pub struct StartConfig {
    pub agent_a_command: String,
    pub agent_b_command: String,
    pub max_rounds: u32,
    pub max_exchanges: u32,
    pub auto_apply: bool,
    pub apply_level: u32,
    pub language: String,
    pub branch_mode: bool,
}

impl MdtalkConfig {
    /// Apply user-chosen start-screen settings, overwriting the relevant fields.
    pub fn apply_start_config(&mut self, sc: StartConfig) {
        self.agent_a.name = sc.agent_a_command.clone();
        self.agent_a.command = sc.agent_a_command;
        self.agent_b.name = sc.agent_b_command.clone();
        self.agent_b.command = sc.agent_b_command;
        self.review.max_rounds = sc.max_rounds;
        self.review.max_exchanges = sc.max_exchanges;
    }

    pub fn load(path: &Path) -> Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("Failed to read {path:?}"))?;
        let config: MdtalkConfig =
            toml::from_str(&content).with_context(|| format!("Failed to parse {path:?}"))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values.
    fn validate(&self) -> Result<()> {
        if self.review.max_rounds < 1 {
            anyhow::bail!("max_rounds 必须 >= 1，当前值为 {}", self.review.max_rounds);
        }
        if self.review.max_exchanges < 1 {
            anyhow::bail!(
                "max_exchanges 必须 >= 1，当前值为 {}",
                self.review.max_exchanges
            );
        }
        Ok(())
    }

    /// Build a config from CLI arguments, falling back to defaults.
    pub fn from_cli(
        project_path: PathBuf,
        agent_a_cmd: Option<String>,
        agent_b_cmd: Option<String>,
        max_rounds: Option<u32>,
        max_exchanges: Option<u32>,
    ) -> Self {
        let agent_a = match agent_a_cmd {
            Some(cmd) => AgentConfig {
                name: cmd.clone(),
                command: cmd,
                timeout_secs: default_timeout(),
            },
            None => default_agent_a(),
        };

        let agent_b = match agent_b_cmd {
            Some(cmd) => AgentConfig {
                name: cmd.clone(),
                command: cmd,
                timeout_secs: default_timeout(),
            },
            None => default_agent_b(),
        };

        Self {
            project: ProjectConfig { path: project_path },
            agent_a,
            agent_b,
            review: ReviewConfig {
                max_rounds: max_rounds.unwrap_or(default_max_rounds()),
                max_exchanges: max_exchanges.unwrap_or(default_max_exchanges()),
                ..Default::default()
            },
            dashboard: DashboardConfig::default(),
        }
    }
}
