use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;

/// Manages reading and writing of the conversation Markdown file.
pub struct Conversation {
    path: PathBuf,
    project_name: String,
}

fn trim_invalid_utf8_prefix(bytes: &[u8]) -> &[u8] {
    // UTF-8 code points are at most 4 bytes. When we tail-read from the file
    // by fixed-size chunks, the first bytes may start in the middle of a code point.
    let max_skip = bytes.len().min(3);
    for skip in 0..=max_skip {
        if std::str::from_utf8(&bytes[skip..]).is_ok() {
            return &bytes[skip..];
        }
    }
    bytes
}

impl Conversation {
    pub fn new(output_dir: &Path, filename: &str, project_name: &str) -> Self {
        Self {
            path: output_dir.join(filename),
            project_name: project_name.to_string(),
        }
    }

    fn append_text(&self, text: &str) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| "Failed to open conversation file for append")?;
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

        let mut file = std::fs::File::open(&self.path)
            .with_context(|| format!("Failed to open conversation file {:?}", self.path))?;
        let file_len = file
            .metadata()
            .with_context(|| format!("Failed to stat conversation file {:?}", self.path))?
            .len();
        if file_len == 0 {
            return Ok(String::new());
        }

        const TAIL_READ_CHUNK_BYTES: usize = 4096;
        let mut pos = file_len;
        let mut newline_count = 0usize;
        let mut total_len = 0usize;
        let mut chunks: Vec<Vec<u8>> = Vec::new();

        // Read from file end until we have enough newline boundaries
        // to reconstruct the last `max_lines` lines.
        while pos > 0 && newline_count <= max_lines {
            let read_len = TAIL_READ_CHUNK_BYTES.min(pos as usize);
            pos -= read_len as u64;
            file.seek(SeekFrom::Start(pos))
                .with_context(|| format!("Failed to seek conversation file {:?}", self.path))?;

            let mut chunk = vec![0u8; read_len];
            file.read_exact(&mut chunk)
                .with_context(|| format!("Failed to read conversation file {:?}", self.path))?;
            newline_count += chunk.iter().filter(|&&b| b == b'\n').count();
            total_len += chunk.len();
            chunks.push(chunk);
        }

        chunks.reverse();
        let mut bytes = Vec::with_capacity(total_len);
        for chunk in chunks {
            bytes.extend_from_slice(&chunk);
        }

        let text = String::from_utf8_lossy(trim_invalid_utf8_prefix(&bytes));
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(max_lines);
        let mut output = lines[start..].join("\n");
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
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut new_file) => {
            if en {
                write!(new_file, "# MDTalk Code Change Log\n\n")?;
            } else {
                write!(new_file, "# MDTalk 代码修改记录\n\n")?;
            }
            new_file
                .flush()
                .with_context(|| format!("Failed to flush changelog header {:?}", path))?;
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => {
            return Err(err)
                .with_context(|| format!("Failed to create changelog header file {:?}", path));
        }
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open changelog {:?}", path))?;

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
    use super::{Conversation, append_changelog_with_language, trim_invalid_utf8_prefix};
    use crate::test_utils::TestTempDir;

    #[test]
    fn read_tail_lines_returns_latest_lines_only() {
        let dir = TestTempDir::new("conversation", "tail-lines");
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
        let dir = TestTempDir::new("conversation", "english-headers");
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

    #[test]
    fn changelog_header_is_only_written_once() {
        let dir = TestTempDir::new("conversation", "changelog-header-once");
        append_changelog_with_language(dir.path(), 1, "change 1", true)
            .expect("failed to append first changelog entry");
        append_changelog_with_language(dir.path(), 2, "change 2", true)
            .expect("failed to append second changelog entry");

        let content = std::fs::read_to_string(dir.path().join("review_changelog.md"))
            .expect("failed to read changelog file");
        assert_eq!(content.matches("# MDTalk Code Change Log").count(), 1);
        assert!(content.contains("## Round 1 Code Changes"));
        assert!(content.contains("## Round 2 Code Changes"));
    }

    #[test]
    fn trim_invalid_utf8_prefix_recovers_from_partial_multibyte_character() {
        let bytes = vec![0x80u8, 0xE4, 0xBD, 0xA0, b'\n'];
        let trimmed = trim_invalid_utf8_prefix(&bytes);
        let text = std::str::from_utf8(trimmed).expect("trimmed bytes should be valid UTF-8");
        assert_eq!(text, "你\n");
    }
}
