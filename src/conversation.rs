use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;

/// Manages reading and writing of the conversation Markdown file.
pub struct Conversation {
    path: PathBuf,
    project_name: String,
}

impl Conversation {
    pub fn new(output_dir: &Path, filename: &str, project_name: &str) -> Self {
        Self {
            path: output_dir.join(filename),
            project_name: project_name.to_string(),
        }
    }

    /// Create the conversation file with the initial header.
    pub fn create(&self) -> Result<()> {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S");
        let header = format!(
            "# 代码审查: {}\n## 审查会话 - {now}\n\n",
            self.project_name
        );
        std::fs::write(&self.path, &header)
            .with_context(|| format!("Failed to create conversation file {:?}", self.path))?;
        Ok(())
    }

    /// Append a round header (### Round N). Called once per round.
    pub fn append_round_header(&self, round: u32) -> Result<()> {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .with_context(|| "Failed to open conversation file for append")?;
        write!(file, "### 第{round}轮\n\n")?;
        Ok(())
    }

    /// Append an agent entry within the current round.
    pub fn append_agent_entry(
        &self,
        agent_name: &str,
        role_label: &str,
        content: &str,
    ) -> Result<()> {
        let now = Local::now().format("%H:%M:%S");
        let entry = format!(
            "#### {agent_name} - {role_label} [{now}]\n\n\
             {content}\n\n---\n\n"
        );
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .with_context(|| "Failed to open conversation file for append")?;
        write!(file, "{entry}")?;
        Ok(())
    }

    /// Append a consensus marker.
    pub fn append_consensus(&self, summary: &str) -> Result<()> {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .with_context(|| "Failed to open conversation file for append")?;
        write!(file, "### 已达成共识 ✓\n\n{summary}\n\n")?;
        Ok(())
    }

    /// Read the full conversation content.
    pub fn read_all(&self) -> Result<String> {
        std::fs::read_to_string(&self.path)
            .with_context(|| format!("Failed to read conversation file {:?}", self.path))
    }
}
