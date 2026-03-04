use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use chrono::Local;

/// Manages reading and writing of the conversation Markdown file.
pub struct Conversation {
    path: PathBuf,
    project_name: String,
    append_file: Mutex<Option<File>>,
}

impl Conversation {
    pub fn new(output_dir: &Path, filename: &str, project_name: &str) -> Self {
        Self {
            path: output_dir.join(filename),
            project_name: project_name.to_string(),
            append_file: Mutex::new(None),
        }
    }

    fn append_text(&self, text: &str) -> Result<()> {
        let mut guard = self
            .append_file
            .lock()
            .map_err(|_| anyhow::anyhow!("Conversation append lock poisoned"))?;
        if guard.is_none() {
            let file = OpenOptions::new()
                .append(true)
                .open(&self.path)
                .with_context(|| "Failed to open conversation file for append")?;
            *guard = Some(file);
        }
        let file = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Conversation append file was not initialized"))?;
        file.write_all(text.as_bytes())
            .with_context(|| "Failed to append to conversation file")?;
        file.flush()
            .with_context(|| "Failed to flush conversation append writes")?;
        Ok(())
    }

    /// Create the conversation file with a localized header.
    pub fn create_with_language(&self, en: bool) -> Result<()> {
        let now = Local::now().format("%Y-%m-%d %H:%M:%S");
        let header = if en {
            format!(
                "# Code Review: {}\n## Review Session - {now}\n\n",
                self.project_name
            )
        } else {
            format!("# 代码审查: {}\n## 审查会话 - {now}\n\n", self.project_name)
        };
        std::fs::write(&self.path, &header)
            .with_context(|| format!("Failed to create conversation file {:?}", self.path))?;

        let mut guard = self
            .append_file
            .lock()
            .map_err(|_| anyhow::anyhow!("Conversation append lock poisoned"))?;
        *guard = None;

        Ok(())
    }

    /// Append a localized round header.
    pub fn append_round_header_with_language(&self, round: u32, en: bool) -> Result<()> {
        let line = if en {
            format!("### Round {round}\n\n")
        } else {
            format!("### 第{round}轮\n\n")
        };
        self.append_text(&line)
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
        self.append_text(&entry)
    }

    /// Append a localized consensus marker.
    pub fn append_consensus_with_language(&self, summary: &str, en: bool) -> Result<()> {
        if en {
            self.append_text(&format!("### Consensus Reached ✓\n\n{summary}\n\n"))
        } else {
            self.append_text(&format!("### 已达成共识 ✓\n\n{summary}\n\n"))
        }
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
pub fn append_changelog_with_language(
    project_dir: &Path,
    round: u32,
    content: &str,
    en: bool,
) -> Result<()> {
    let path = project_dir.join("review_changelog.md");
    let needs_header = !path.exists();

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open changelog {:?}", path))?;

    if needs_header {
        if en {
            write!(file, "# MDTalk Code Change Log\n\n")?;
        } else {
            write!(file, "# MDTalk 代码修改记录\n\n")?;
        }
    }

    let now = Local::now().format("%Y-%m-%d %H:%M:%S");
    if en {
        write!(
            file,
            "## Round {round} Code Changes - {now}\n\n{content}\n\n---\n\n"
        )?;
    } else {
        write!(
            file,
            "## 第{round}轮 代码修改 - {now}\n\n{content}\n\n---\n\n"
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::Conversation;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    struct TestTempDir {
        path: PathBuf,
    }

    impl TestTempDir {
        fn new(name: &str) -> Self {
            let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("mdtalk-conv-{name}-{}-{id}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("failed to create test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn read_tail_lines_returns_latest_lines_only() {
        let dir = TestTempDir::new("tail-lines");
        let conv = Conversation::new(dir.path(), "conversation.md", "test");
        conv.create_with_language(false)
            .expect("failed to create conversation file");

        let mut all_lines = String::new();
        for i in 0..20 {
            all_lines.push_str(&format!("line-{i}\n"));
        }
        std::fs::write(dir.path().join("conversation.md"), all_lines)
            .expect("failed to write test lines");

        let tail = conv.read_tail_lines(5).expect("failed to read tail lines");
        assert!(!tail.contains("line-0"));
        assert!(tail.contains("line-15"));
        assert!(tail.contains("line-19"));
    }

    #[test]
    fn english_headers_are_written_when_localized() {
        let dir = TestTempDir::new("english-headers");
        let conv = Conversation::new(dir.path(), "conversation.md", "test-project");
        conv.create_with_language(true)
            .expect("failed to create english conversation file");
        conv.append_round_header_with_language(1, true)
            .expect("failed to append english round header");
        conv.append_consensus_with_language("All aligned.", true)
            .expect("failed to append english consensus");

        let content = std::fs::read_to_string(dir.path().join("conversation.md"))
            .expect("failed to read conversation");
        assert!(content.contains("# Code Review: test-project"));
        assert!(content.contains("## Review Session - "));
        assert!(content.contains("### Round 1"));
        assert!(content.contains("### Consensus Reached ✓"));
    }
}
