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
    900
}
fn default_max_rounds() -> u32 {
    1
}
fn default_max_exchanges() -> u32 {
    5
}
fn default_consensus_keywords() -> Vec<String> {
    vec![
        // ── English: full agreement ────────────────────────────────────────
        "agree".to_string(),
        "consensus".to_string(),
        "no further".to_string(),
        "looks good".to_string(),
        "sounds good".to_string(),
        "LGTM".to_string(),
        "confirmed".to_string(),
        "accepted".to_string(),
        "acknowledged".to_string(),
        "approved".to_string(),
        "correct".to_string(),
        "valid".to_string(),
        "verified".to_string(),
        "concur".to_string(),
        // ── English: partial agreement ─────────────────────────────────────
        "partially agree".to_string(),
        "partially confirmed".to_string(),
        "partially accepted".to_string(),
        "mostly agree".to_string(),
        // ── English: conclusion line (mandatory format in prompts) ─────────
        "CONCLUSION: I agree".to_string(),
        "CONCLUSION: agree".to_string(),
        "CONCLUSION: confirmed".to_string(),
        "CONCLUSION: accepted".to_string(),
        "CONCLUSION: partially agree".to_string(),
        "CONCLUSION: partially confirmed".to_string(),
        "CONCLUSION: mostly agree".to_string(),
        // ── Chinese: full agreement ────────────────────────────────────────
        "同意".to_string(),
        "达成一致".to_string(),
        "认可".to_string(),
        "确认".to_string(),
        "成立".to_string(),
        "正确".to_string(),
        "有道理".to_string(),
        "说得对".to_string(),
        "没问题".to_string(),
        // ── Chinese: partial agreement ─────────────────────────────────────
        "部分同意".to_string(),
        "部分成立".to_string(),
        "部分认可".to_string(),
        "部分确认".to_string(),
        // ── Chinese: conclusion line (mandatory format in prompts) ─────────
        "结论：同意".to_string(),
        "结论：认可".to_string(),
        "结论：成立".to_string(),
        "结论：确认".to_string(),
        "结论：部分同意".to_string(),
        "结论：部分成立".to_string(),
        "结论：部分认可".to_string(),
        "结论：部分确认".to_string(),
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
    pub agent_a_timeout_secs: u64,
    pub agent_b_timeout_secs: u64,
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
        self.agent_a.timeout_secs = sc.agent_a_timeout_secs;
        self.agent_b.name = sc.agent_b_command.clone();
        self.agent_b.command = sc.agent_b_command;
        self.agent_b.timeout_secs = sc.agent_b_timeout_secs;
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

    /// Build config from project path:
    /// - if `<project>/mdtalk.toml` exists, load it
    /// - otherwise fall back to defaults
    ///
    /// In both cases, `project.path` is forced to the CLI project path.
    pub fn from_project_with_optional_config(project_path: PathBuf) -> Result<Self> {
        let config_path = project_path.join("mdtalk.toml");
        let mut cfg = if config_path.is_file() {
            Self::load(&config_path)?
        } else {
            Self::from_cli(project_path.clone(), None, None, None, None)
        };
        cfg.project.path = project_path;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Apply CLI overrides on top of the current config.
    /// Priority: defaults/file < CLI.
    pub fn apply_cli_overrides(
        &mut self,
        project_path: Option<PathBuf>,
        agent_a_cmd: Option<String>,
        agent_b_cmd: Option<String>,
        max_rounds: Option<u32>,
        max_exchanges: Option<u32>,
    ) -> Result<()> {
        if let Some(path) = project_path {
            self.project.path = path;
        }

        if let Some(cmd) = agent_a_cmd {
            self.agent_a.name = cmd.clone();
            self.agent_a.command = cmd;
        }

        if let Some(cmd) = agent_b_cmd {
            self.agent_b.name = cmd.clone();
            self.agent_b.command = cmd;
        }

        if let Some(v) = max_rounds {
            self.review.max_rounds = v;
        }

        if let Some(v) = max_exchanges {
            self.review.max_exchanges = v;
        }

        self.validate()
    }

    /// Validate configuration values.
    fn validate(&self) -> Result<()> {
        if self.agent_a.timeout_secs == 0 {
            anyhow::bail!("agent_a.timeout_secs 必须 >= 1，当前值为 0");
        }
        if self.agent_b.timeout_secs == 0 {
            anyhow::bail!("agent_b.timeout_secs 必须 >= 1，当前值为 0");
        }
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::MdtalkConfig;
    use crate::test_utils::TestTempDir;

    #[test]
    fn cli_overrides_loaded_config_values() {
        let mut cfg = MdtalkConfig::from_cli(PathBuf::from("."), None, None, Some(1), Some(1));
        cfg.agent_a.timeout_secs = 900;
        cfg.agent_b.timeout_secs = 1200;

        cfg.apply_cli_overrides(
            Some(PathBuf::from("C:/tmp/project")),
            Some("codex".to_string()),
            Some("claude".to_string()),
            Some(3),
            Some(7),
        )
        .expect("cli overrides should be valid");

        assert_eq!(cfg.project.path, PathBuf::from("C:/tmp/project"));
        assert_eq!(cfg.agent_a.command, "codex");
        assert_eq!(cfg.agent_b.command, "claude");
        assert_eq!(cfg.review.max_rounds, 3);
        assert_eq!(cfg.review.max_exchanges, 7);
        // Overriding command should not reset timeout from existing config.
        assert_eq!(cfg.agent_a.timeout_secs, 900);
        assert_eq!(cfg.agent_b.timeout_secs, 1200);
    }

    #[test]
    fn cli_override_rejects_zero_agent_a_timeout() {
        let mut cfg = MdtalkConfig::from_cli(PathBuf::from("."), None, None, Some(1), Some(1));
        cfg.agent_a.timeout_secs = 0;

        let result = cfg.apply_cli_overrides(None, None, None, None, None);
        assert!(
            result.is_err(),
            "validation should reject agent_a timeout of 0 seconds"
        );
    }

    #[test]
    fn cli_override_rejects_zero_agent_b_timeout() {
        let mut cfg = MdtalkConfig::from_cli(PathBuf::from("."), None, None, Some(1), Some(1));
        cfg.agent_b.timeout_secs = 0;

        let result = cfg.apply_cli_overrides(None, None, None, None, None);
        assert!(
            result.is_err(),
            "validation should reject agent_b timeout of 0 seconds"
        );
    }

    #[test]
    fn project_loader_prefers_local_mdtalk_toml() {
        let dir = TestTempDir::new("config", "load-local-toml");
        let toml = r#"
[project]
path = "."

[agent_a]
name = "claude"
command = "claude"
timeout_secs = 901

[agent_b]
name = "codex"
command = "codex"
timeout_secs = 902

[review]
max_rounds = 2
max_exchanges = 3
output_file = "conversation.md"
consensus_keywords = ["agree"]
"#;
        std::fs::write(dir.path().join("mdtalk.toml"), toml).expect("failed to write mdtalk.toml");

        let cfg = MdtalkConfig::from_project_with_optional_config(dir.path().to_path_buf())
            .expect("project loader should read local mdtalk.toml");

        assert_eq!(cfg.project.path, dir.path().to_path_buf());
        assert_eq!(cfg.agent_a.timeout_secs, 901);
        assert_eq!(cfg.agent_b.timeout_secs, 902);
        assert_eq!(cfg.review.max_rounds, 2);
        assert_eq!(cfg.review.max_exchanges, 3);
    }

    #[test]
    fn project_loader_falls_back_to_defaults_without_local_toml() {
        let dir = TestTempDir::new("config", "load-defaults");

        let cfg = MdtalkConfig::from_project_with_optional_config(dir.path().to_path_buf())
            .expect("project loader should fall back to defaults");

        assert_eq!(cfg.project.path, dir.path().to_path_buf());
        assert_eq!(cfg.agent_a.command, "claude");
        assert_eq!(cfg.agent_b.command, "codex");
        assert_eq!(cfg.agent_a.timeout_secs, 900);
        assert_eq!(cfg.agent_b.timeout_secs, 900);
    }
}
