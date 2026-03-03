use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::{info, warn};

use crate::config::AgentConfig;

#[derive(Debug, Clone)]
pub struct AgentOutput {
    pub content: String,
    pub duration: Duration,
}

pub struct AgentRunner {
    pub name: String,
    command: String,
    timeout: Duration,
}

impl AgentRunner {
    pub fn new(config: &AgentConfig) -> Self {
        Self {
            name: config.name.clone(),
            command: config.command.clone(),
            timeout: Duration::from_secs(config.timeout_secs),
        }
    }

    /// Build the CLI arguments depending on which agent we're calling.
    fn build_args(&self, prompt: &str) -> Vec<String> {
        match self.command.as_str() {
            "claude" => vec![
                "-p".to_string(),
                prompt.to_string(),
                "--output-format".to_string(),
                "text".to_string(),
            ],
            "codex" => vec![
                "exec".to_string(),
                "--full-auto".to_string(),
                prompt.to_string(),
            ],
            // Generic fallback: just pass prompt as a single arg
            _ => vec![prompt.to_string()],
        }
    }

    /// Run the agent with the given prompt, in the given project directory.
    pub async fn run(&self, prompt: &str, project_path: &Path) -> Result<AgentOutput> {
        let args = self.build_args(prompt);
        info!(
            agent = %self.name,
            command = %self.command,
            "Starting agent"
        );

        let start = Instant::now();

        // On Windows, CLI tools installed via npm are .cmd scripts that need
        // to be invoked through cmd.exe for proper PATH resolution.
        let mut child = if cfg!(windows) {
            let mut full_args = vec!["/C".to_string(), self.command.clone()];
            full_args.extend(args.clone());
            Command::new("cmd")
                .args(&full_args)
                .current_dir(project_path)
                .env_remove("CLAUDECODE")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .with_context(|| format!("Failed to spawn agent '{}'", self.name))?
        } else {
            Command::new(&self.command)
                .args(&args)
                .current_dir(project_path)
                .env_remove("CLAUDECODE")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .with_context(|| format!("Failed to spawn agent '{}'", self.name))?
        };

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
                let stdout_bytes = stdout_task.await
                    .map_err(|e| anyhow::anyhow!("stdout task join error: {e}"))?
                    .map_err(|e| anyhow::anyhow!("stdout read error: {e}"))?;
                let stderr_bytes = stderr_task.await
                    .map_err(|e| anyhow::anyhow!("stderr task join error: {e}"))?
                    .map_err(|e| anyhow::anyhow!("stderr read error: {e}"))?;

                let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
                let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();

                if !stderr.is_empty() {
                    warn!(agent = %self.name, stderr = %stderr, "Agent produced stderr output");
                }

                if status.code() != Some(0) && stdout.trim().is_empty() {
                    anyhow::bail!(
                        "Agent '{}' exited with code {:?} and produced no output. stderr: {}",
                        self.name,
                        status.code(),
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
                if let Some(pid) = child.id() {
                    if cfg!(windows) {
                        let _ = Command::new("taskkill")
                            .args(["/T", "/F", "/PID", &pid.to_string()])
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status()
                            .await;
                    } else {
                        child.kill().await.ok();
                    }
                } else {
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
