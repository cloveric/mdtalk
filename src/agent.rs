use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::{info, warn};

use crate::config::AgentConfig;
#[cfg(unix)]
use nix::sys::signal::{Signal, killpg};
#[cfg(unix)]
use nix::unistd::Pid;

#[derive(Debug, Clone)]
pub struct AgentOutput {
    pub content: String,
    pub duration: Duration,
}

pub struct AgentRunner {
    pub name: String,
    command: String,
    command_program: String,
    command_prefix_args: Vec<String>,
    timeout: Duration,
}

impl AgentRunner {
    pub fn new(config: &AgentConfig) -> Self {
        let parts = split_command_line(&config.command);
        let command_program = parts
            .first()
            .cloned()
            .unwrap_or_else(|| config.command.clone());
        let command_prefix_args = parts.into_iter().skip(1).collect();
        Self {
            name: config.name.clone(),
            command: config.command.clone(),
            command_program,
            command_prefix_args,
            timeout: Duration::from_secs(config.timeout_secs),
        }
    }

    /// Build the CLI arguments depending on which agent we're calling.
    fn build_args(&self, prompt: &str) -> Vec<String> {
        match command_name_from_program(&self.command_program).as_str() {
            "claude" => vec![
                "-p".to_string(),
                prompt.to_string(),
                "--output-format".to_string(),
                "text".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ],
            "codex" => vec![
                "exec".to_string(),
                "--dangerously-bypass-approvals-and-sandbox".to_string(),
                prompt.to_string(),
            ],
            "gemini" => vec!["--approval-mode=yolo".to_string(), prompt.to_string()],
            // Generic fallback: just pass prompt as a single arg
            _ => vec![prompt.to_string()],
        }
    }

    /// Run the agent with the given prompt, in the given project directory.
    pub async fn run(&self, prompt: &str, project_path: &Path) -> Result<AgentOutput> {
        let mut args = self.command_prefix_args.clone();
        args.extend(self.build_args(prompt));
        info!(
            agent = %self.name,
            command = %self.command,
            "Starting agent"
        );

        let start = Instant::now();

        let (command_program, command_args) = if cfg!(windows) {
            // On Windows, CLI tools installed via npm are often .cmd scripts that
            // are more reliable when launched through cmd /C.
            let mut full_args = vec!["/C".to_string(), self.command_program.clone()];
            full_args.extend(args);
            ("cmd".to_string(), full_args)
        } else {
            (self.command_program.clone(), args)
        };

        let mut cmd = Command::new(&command_program);
        cmd.kill_on_drop(true)
            .args(&command_args)
            .current_dir(project_path)
            .env_remove("CLAUDECODE")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn agent '{}'", self.name))?;

        // Read stdout/stderr concurrently with wait to avoid deadlock
        // when pipe buffers fill up.
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            if let Some(mut out) = stdout_handle {
                tokio::io::AsyncReadExt::read_to_end(&mut out, &mut bytes).await?;
            }
            Ok::<_, std::io::Error>(bytes)
        });

        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            if let Some(mut err) = stderr_handle {
                tokio::io::AsyncReadExt::read_to_end(&mut err, &mut bytes).await?;
            }
            Ok::<_, std::io::Error>(bytes)
        });

        // Wait for the child process (but NOT the read tasks yet - those are separate)
        let wait_result = tokio::time::timeout(self.timeout, child.wait()).await;
        let duration = start.elapsed();

        match wait_result {
            Ok(Ok(status)) => {
                // Process exited, now collect stdout/stderr (should complete quickly)
                let stdout_bytes = stdout_task
                    .await
                    .map_err(|e| anyhow::anyhow!("stdout task join error: {e}"))?
                    .map_err(|e| anyhow::anyhow!("stdout read error: {e}"))?;
                let stderr_bytes = stderr_task
                    .await
                    .map_err(|e| anyhow::anyhow!("stderr task join error: {e}"))?
                    .map_err(|e| anyhow::anyhow!("stderr read error: {e}"))?;

                let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
                let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();

                if !stderr.is_empty() {
                    warn!(agent = %self.name, stderr = %stderr, "Agent produced stderr output");
                }

                if !status.success() {
                    anyhow::bail!(
                        "Agent '{}' 以退出码 {:?} 异常退出。\nstderr: {}\nstdout (前500字符): {}",
                        self.name,
                        status.code(),
                        stderr.chars().take(500).collect::<String>(),
                        stdout.chars().take(500).collect::<String>()
                    );
                }

                if stdout.trim().is_empty() {
                    anyhow::bail!(
                        "Agent '{}' 正常退出但未产生任何输出。stderr: {}",
                        self.name,
                        stderr.chars().take(500).collect::<String>()
                    );
                }

                info!(
                    agent = %self.name,
                    exit_code = ?status.code(),
                    duration_secs = duration.as_secs(),
                    output_len = stdout.len(),
                    "Agent completed"
                );

                Ok(AgentOutput {
                    content: stdout,
                    duration,
                })
            }
            Ok(Err(e)) => {
                stdout_task.abort();
                stderr_task.abort();
                anyhow::bail!("Agent '{}' process error: {}", self.name, e);
            }
            Err(_) => {
                // Timeout: kill the entire process tree, then abort read tasks
                warn!(agent = %self.name, timeout_secs = self.timeout.as_secs(), "Agent timed out, killing process");

                // On Windows, child.kill() only kills cmd.exe, not the actual agent.
                // Use taskkill /T /F /PID to kill the entire process tree.
                #[cfg(windows)]
                {
                    if let Some(pid) = child.id() {
                        let _ = Command::new("taskkill")
                            .args(["/T", "/F", "/PID", &pid.to_string()])
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status()
                            .await;
                    }
                    child.kill().await.ok();
                }

                #[cfg(unix)]
                {
                    if let Some(pid) = child.id() {
                        // The child is started with process_group(0), so pid == pgid.
                        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
                    }
                    child.kill().await.ok();
                }

                #[cfg(not(any(unix, windows)))]
                {
                    child.kill().await.ok();
                }

                // Abort the read tasks since the process is dead
                stdout_task.abort();
                stderr_task.abort();

                anyhow::bail!(
                    "Agent '{}' 超时 ({}秒)，已终止",
                    self.name,
                    self.timeout.as_secs()
                );
            }
        }
    }
}

fn command_name_from_program(program: &str) -> String {
    std::path::Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase()
}

fn split_command_line(command: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let parts = winsplit::split(command);
        if parts.is_empty() {
            vec![command.to_string()]
        } else {
            parts
        }
    }

    #[cfg(not(windows))]
    {
        match shell_words::split(command) {
            Ok(parts) if !parts.is_empty() => parts,
            _ => vec![command.to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::AgentConfig;

    use super::AgentRunner;

    fn runner(command: &str) -> AgentRunner {
        let cfg = AgentConfig {
            name: command.to_string(),
            command: command.to_string(),
            timeout_secs: 60,
        };
        AgentRunner::new(&cfg)
    }

    #[test]
    fn codex_uses_dangerous_bypass_mode() {
        let args = runner("codex").build_args("hello");
        assert!(
            args.iter()
                .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
        );
    }

    #[test]
    fn claude_uses_dangerous_skip_permissions() {
        let args = runner("claude").build_args("hello");
        assert!(
            args.iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );
    }

    #[test]
    fn gemini_uses_yolo_mode() {
        let args = runner("gemini").build_args("hello");
        assert!(args.iter().any(|arg| arg == "--approval-mode=yolo"));
    }

    #[test]
    fn codex_with_extra_cli_args_still_uses_exec_mode() {
        let args = runner("codex --model gpt-5").build_args("hello");
        assert!(args.iter().any(|arg| arg == "exec"));
        assert!(
            args.iter()
                .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
        );
    }

    #[test]
    fn command_prefix_args_are_parsed_from_command_line() {
        let runner = runner("codex --model gpt-5");
        assert_eq!(runner.command_program, "codex");
        assert_eq!(
            runner.command_prefix_args,
            vec!["--model".to_string(), "gpt-5".to_string()]
        );
    }
}
