use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
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
        let header = format!("# 代码审查: {}\n## 审查会话 - {now}\n\n", self.project_name);
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

    /// Read only the last `max_lines` lines of the conversation file.
    pub fn read_tail_lines(&self, max_lines: usize) -> Result<String> {
        if max_lines == 0 {
            return Ok(String::new());
        }

        let file = std::fs::File::open(&self.path)
            .with_context(|| format!("Failed to open conversation file {:?}", self.path))?;
        let reader = BufReader::new(file);
        let mut tail = VecDeque::with_capacity(max_lines);

        for line in reader.lines() {
            let line =
                line.with_context(|| format!("Failed to read conversation file {:?}", self.path))?;
            if tail.len() == max_lines {
                tail.pop_front();
            }
            tail.push_back(line);
        }

        let mut output = tail.into_iter().collect::<Vec<_>>().join("\n");
        if !output.is_empty() {
            output.push('\n');
        }
        Ok(output)
    }
}

/// Append an apply-phase entry to `review_changelog.md` in the project directory.
/// Creates the file with a header on first write.
pub fn append_changelog(project_dir: &Path, round: u32, content: &str) -> Result<()> {
    let path = project_dir.join("review_changelog.md");
    let needs_header = !path.exists();

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open changelog {:?}", path))?;

    if needs_header {
        write!(file, "# MDTalk 代码修改记录\n\n")?;
    }

    let now = Local::now().format("%Y-%m-%d %H:%M:%S");
    write!(
        file,
        "## 第{round}轮 代码修改 - {now}\n\n{content}\n\n---\n\n"
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::Conversation;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mdtalk-conv-{name}-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("failed to create test dir");
        dir
    }

    #[test]
    fn read_tail_lines_returns_latest_lines_only() {
        let dir = unique_test_dir("tail-lines");
        let conv = Conversation::new(&dir, "conversation.md", "test");
        conv.create().expect("failed to create conversation file");

        let mut all_lines = String::new();
        for i in 0..20 {
            all_lines.push_str(&format!("line-{i}\n"));
        }
        std::fs::write(dir.join("conversation.md"), all_lines).expect("failed to write test lines");

        let tail = conv.read_tail_lines(5).expect("failed to read tail lines");
        assert!(!tail.contains("line-0"));
        assert!(tail.contains("line-15"));
        assert!(tail.contains("line-19"));

        let _ = std::fs::remove_dir_all(dir);
    }
}
