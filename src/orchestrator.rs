use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::{mpsc, watch};
use tracing::{error, info};

use tokio::process::Command as TokioCommand;

use crate::agent::{AgentOutput, AgentRunMode, AgentRunner};
use crate::config::MdtalkConfig;
use crate::consensus;
use crate::conversation::Conversation;

/// Get current git branch name. Returns None if not a git repo.
async fn git_current_branch(project_path: &Path) -> Option<String> {
    let output = TokioCommand::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Create and switch to a new git branch.
async fn git_checkout_new_branch(project_path: &Path, branch_name: &str) -> Result<()> {
    let output = TokioCommand::new("git")
        .args(["checkout", "-b", branch_name])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git checkout -b {branch_name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

async fn git_checkout_branch(project_path: &Path, branch_name: &str) -> Result<()> {
    let output = TokioCommand::new("git")
        .args(["checkout", branch_name])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git checkout {branch_name} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn git_delete_branch(project_path: &Path, branch_name: &str) -> Result<()> {
    let output = TokioCommand::new("git")
        .args(["branch", "-D", branch_name])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git branch -D {branch_name} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Collect changed paths from git status, including untracked files.
async fn git_status_paths(project_path: &Path) -> Result<HashSet<String>> {
    let output = TokioCommand::new("git")
        .args(["status", "--porcelain", "-z"])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git status --porcelain -z failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let mut paths = HashSet::new();
    let mut entries = output
        .stdout
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty());

    while let Some(entry) = entries.next() {
        if entry.len() < 3 || entry[2] != b' ' {
            continue;
        }

        let status_x = entry[0] as char;
        let status_y = entry[1] as char;
        let path = decode_git_status_path(&entry[3..])?;
        if !path.is_empty() {
            paths.insert(path);
        }

        // In porcelain -z mode, renames/copies are followed by an extra path token.
        if (matches!(status_x, 'R' | 'C') || matches!(status_y, 'R' | 'C'))
            && let Some(next_path) = entries.next()
        {
            let next = decode_git_status_path(next_path)?;
            if !next.is_empty() {
                paths.insert(next);
            }
        }
    }

    Ok(paths)
}

fn decode_git_status_path(path_bytes: &[u8]) -> Result<String> {
    let path = std::str::from_utf8(path_bytes).map_err(|_| {
        anyhow::anyhow!(
            "git status --porcelain -z returned a non-UTF-8 path, \
             which is unsupported in filtered-commit mode"
        )
    })?;
    Ok(path.to_string())
}

/// Stage the provided paths.
async fn git_add_paths(project_path: &Path, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut cmd = TokioCommand::new("git");
    cmd.arg("add");
    cmd.arg("--");
    cmd.args(paths);
    let output = cmd
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git add (selected paths) failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Check whether staged changes exist for the provided paths.
async fn git_has_staged_changes_for_paths(project_path: &Path, paths: &[String]) -> Result<bool> {
    if paths.is_empty() {
        return Ok(false);
    }

    let mut cmd = TokioCommand::new("git");
    cmd.args(["diff", "--cached", "--quiet", "--"]);
    cmd.args(paths);
    let output = cmd
        .current_dir(project_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => anyhow::bail!(
            "git diff --cached --quiet -- <paths> failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

/// Commit only the provided paths without touching unrelated staged entries.
async fn git_commit_only_paths(project_path: &Path, message: &str, paths: &[String]) -> Result<()> {
    let mut cmd = TokioCommand::new("git");
    cmd.args(["commit", "-m", message, "--only", "--"]);
    cmd.args(paths);
    let commit_output = cmd
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr)
            .trim()
            .to_string();
        let stdout = String::from_utf8_lossy(&commit_output.stdout)
            .trim()
            .to_string();
        let details = if stderr.is_empty() { stdout } else { stderr };
        anyhow::bail!("git commit -m failed: {details}");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitCommitOutcome {
    Committed,
    NothingToCommit,
}

/// Stage and commit changes while excluding paths that were already dirty before session start.
async fn git_commit_filtered(
    project_path: &Path,
    message: &str,
    excluded_paths: &HashSet<String>,
) -> Result<GitCommitOutcome> {
    let mut paths_to_stage: Vec<String> = git_status_paths(project_path)
        .await?
        .into_iter()
        .filter(|path| !excluded_paths.contains(path))
        .collect();
    paths_to_stage.sort_unstable();
    if paths_to_stage.is_empty() {
        return Ok(GitCommitOutcome::NothingToCommit);
    }

    git_add_paths(project_path, &paths_to_stage).await?;

    if !git_has_staged_changes_for_paths(project_path, &paths_to_stage).await? {
        return Ok(GitCommitOutcome::NothingToCommit);
    }

    git_commit_only_paths(project_path, message, &paths_to_stage).await?;

    Ok(GitCommitOutcome::Committed)
}

/// Checkout target branch and merge the given branch into it.
async fn git_checkout_and_merge(
    project_path: &Path,
    target_branch: &str,
    merge_branch: &str,
) -> Result<()> {
    let output = TokioCommand::new("git")
        .args(["checkout", target_branch])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git checkout {target_branch} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = TokioCommand::new("git")
        .args(["merge", merge_branch])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        let merge_err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let abort_output = TokioCommand::new("git")
            .args(["merge", "--abort"])
            .current_dir(project_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await?;
        if !abort_output.status.success() {
            let abort_err = String::from_utf8_lossy(&abort_output.stderr)
                .trim()
                .to_string();
            let lower = abort_err.to_ascii_lowercase();
            let no_merge_in_progress = lower.contains("no merge to abort")
                || lower.contains("there is no merge to abort")
                || lower.contains("merge_head missing");
            if !no_merge_in_progress {
                anyhow::bail!(
                    "git merge {merge_branch} failed: {merge_err}; git merge --abort failed: {abort_err}"
                );
            }
        }
        anyhow::bail!("git merge {merge_branch} failed: {}", merge_err);
    }
    Ok(())
}

/// Run an agent while sending heartbeat logs to the dashboard every 30 seconds.
async fn run_agent_with_heartbeat(
    agent: &AgentRunner,
    prompt: &str,
    project_path: &Path,
    mode: AgentRunMode,
    label: &str,
    state: &mut OrchestratorState,
    state_tx: &watch::Sender<OrchestratorState>,
) -> Result<AgentOutput> {
    let start = Instant::now();
    let agent_fut = agent.run_with_mode(prompt, project_path, mode);
    tokio::pin!(agent_fut);

    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    heartbeat.tick().await; // skip the first immediate tick

    loop {
        tokio::select! {
            result = &mut agent_fut => {
                return result;
            }
            _ = heartbeat.tick() => {
                let elapsed = start.elapsed().as_secs();
                state.log(&if state.is_en() {
                    format!("{label} running... ({elapsed}s)")
                } else {
                    format!("{label} 运行中... (已{elapsed}秒)")
                });
                let _ = state_tx.send(state.clone());
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExchangeKind {
    InitialReview,
    RoundReReview,
    FollowUp,
}

fn classify_exchange(round: u32, exchange: u32) -> ExchangeKind {
    if round == 1 && exchange == 1 {
        ExchangeKind::InitialReview
    } else if exchange == 1 {
        ExchangeKind::RoundReReview
    } else {
        ExchangeKind::FollowUp
    }
}

fn should_append_round_header(exchange: u32) -> bool {
    exchange == 1
}

fn build_agent_a_prompt(
    exchange_kind: ExchangeKind,
    conv_filename: &str,
    previous_round_code_modified: bool,
    en: bool,
) -> String {
    if en {
        return match exchange_kind {
            ExchangeKind::InitialReview => "You are participating in a multi-agent code review workflow. \
Please thoroughly read the project's source files (src/ directory) and provide a detailed review, including:\n\
- Potential bugs and logic errors\n\
- Code quality issues\n\
- Architecture/design issues\n\
- Improvement suggestions\n\n\
Prioritize findings by severity.\n\n\
CRITICAL: Do NOT modify any source code files. Only review and report issues."
                .to_string(),
            ExchangeKind::RoundReReview => {
                let modification_context = if previous_round_code_modified {
                    "Code was modified after the previous round."
                } else {
                    "No code changes were applied after the previous round."
                };
                format!(
                    "You are participating in a multi-agent code review workflow. \
{modification_context} \
First read the full review history in {conv_filename}, then re-review src/ to verify previously identified issues are fixed and to detect any newly introduced issues.\n\n\
CRITICAL: Do NOT modify any source code files. Only review and report issues."
                )
            }
            ExchangeKind::FollowUp => format!(
                "You are Agent A in a multi-agent code review debate. \
Read {conv_filename} and focus ONLY on Agent B's LAST response.\n\n\
DO NOT summarize the conversation. DO NOT repeat previous points. \
Respond DIRECTLY to each item in B's last response:\n\n\
For each item B marked as [Agree]/[Disagree]/[Partially Agree]:\n\
- If B disagreed: either provide counter-evidence from source code to defend your position, or concede\n\
- If B partially agreed: address the specific concern B raised\n\
- If B agreed: no need to repeat, just move on\n\
- If B raised new issues: evaluate them against actual code\n\n\
CRITICAL: Do NOT modify any source code files. Do NOT summarize history.\n\n\
You MUST end your response with one of these exact conclusion lines:\n\
  If you fully agree with B's assessment: CONCLUSION: I agree\n\
  If you partially agree: CONCLUSION: partially agree\n\
  If you disagree: CONCLUSION: I disagree"
            ),
        };
    }

    match exchange_kind {
        ExchangeKind::InitialReview => "你正在参与一个多 agent 代码审查流程。\
请仔细阅读当前项目的所有源代码文件（src/ 目录），然后给出详细的审查意见，包括：\n\
- 潜在的 bug 和逻辑错误\n\
- 代码质量问题\n\
- 架构设计问题\n\
- 改进建议\n\n\
请按优先级排列你的发现。\n\n\
严禁修改任何源代码文件！只进行审查和报告问题。"
            .to_string(),
        ExchangeKind::RoundReReview => {
            let modification_context = if previous_round_code_modified {
                "上一轮审查后代码已被修改。"
            } else {
                "上一轮审查后没有执行代码修改。"
            };
            format!(
                "你正在参与一个多 agent 代码审查流程。\
{modification_context}\
请先阅读当前目录下的 {conv_filename} 文件了解完整的审查对话历史，\
然后重新审查 src/ 目录下的源代码，检查之前发现的问题是否已修复，\
以及是否引入了新问题。给出你的审查意见。\n\n\
严禁修改任何源代码文件！只进行审查和报告问题。"
            )
        }
        ExchangeKind::FollowUp => format!(
            "你是 Agent A，正在进行多 agent 代码审查辩论。\
请阅读 {conv_filename}，只关注 Agent B 的最后一条回复。\n\n\
禁止总结对话历史！禁止重复已讨论过的内容！\
直接针对 B 最后一条回复中的每一条逐项回应：\n\n\
- B 标注【不同意】的条目：用源代码中的证据反驳，或接受 B 的判断\n\
- B 标注【部分成立】的条目：针对 B 提出的具体顾虑进行回应\n\
- B 标注【同意】的条目：无需重复，跳过即可\n\
- B 提出的新问题：对照实际代码评估\n\n\
严禁修改任何源代码文件！禁止总结历史！\n\n\
你必须在回复末尾单独写一行结论（格式固定，不可省略）：\n\
  完全同意 B 的评估：结论：同意\n\
  部分同意：结论：部分同意\n\
  存在分歧：结论：不同意"
        ),
    }
}

fn build_agent_b_prompt(exchange_kind: ExchangeKind, conv_filename: &str, en: bool) -> String {
    if en {
        return match exchange_kind {
            ExchangeKind::InitialReview | ExchangeKind::RoundReReview => format!(
                "You are an independent code review expert. Verify each finding recorded in '{conv_filename}'.\n\n\
Steps:\n\
1. Read '{conv_filename}' and list all findings from the other reviewer\n\
2. Open the related source files and verify each finding against actual code\n\
3. Output your complete review response directly in this format:\n\
   - Mark each finding as [Agree] or [Disagree], with concrete code evidence\n\
   - Add any missed issues\n\
   - End with a mandatory conclusion line (this line is required and must appear exactly):\n\
     Full agreement: write exactly → CONCLUSION: I agree\n\
     Partial agreement: write exactly → CONCLUSION: partially agree\n\
     Disagree: write exactly → CONCLUSION: I disagree\n\n\
CRITICAL: Do NOT modify any source code files. Your role is to verify and discuss only. \
Code changes will be applied in a separate phase after consensus is reached.\n\n\
Important: output the full review content directly; do not only report which files you read."
            ),
            ExchangeKind::FollowUp => format!(
                "You are Agent B in a multi-agent code review debate. \
Read {conv_filename} and focus ONLY on Agent A's LAST response.\n\n\
DO NOT summarize the conversation. DO NOT repeat previous points. \
Respond DIRECTLY to each item in A's last response:\n\n\
- If A defended a finding you disagreed with: evaluate A's new evidence against actual source code\n\
- If A conceded a point: acknowledge briefly\n\
- If A raised new issues: verify them against actual code\n\
- If you still disagree with A: explain why with specific code references\n\n\
CRITICAL: Do NOT modify any source code files. Do NOT summarize history.\n\n\
You MUST end your response with one of these exact conclusion lines:\n\
  If you fully agree with A's position: CONCLUSION: I agree\n\
  If you partially agree: CONCLUSION: partially agree\n\
  If you disagree: CONCLUSION: I disagree"
            ),
        };
    }

    match exchange_kind {
        ExchangeKind::InitialReview | ExchangeKind::RoundReReview => format!(
            "你是一位独立的代码审查专家。你的任务是对 '{conv_filename}' 中记录的代码审查意见进行逐条验证。\n\n\
具体步骤：\n\
1. 读取 '{conv_filename}' 文件，找到另一位审查者提出的所有发现\n\
2. 对每一条发现，打开对应的源代码文件，核实该问题是否真实存在\n\
3. 直接输出你的完整审查回应，格式如下：\n\
   - 对每条发现标注【同意】或【不同意】，附上你在源代码中看到的证据\n\
   - 补充任何审查者遗漏的新问题\n\
   - 在最后必须单独写一行结论（此行为强制要求，格式固定）：\n\
     完全认可：写 → 结论：同意\n\
     部分认可：写 → 结论：部分同意\n\
     存在重大分歧：写 → 结论：不同意\n\n\
严禁修改任何源代码文件！你的职责仅限于验证和讨论。代码修改将在达成共识后的专门阶段执行。\n\n\
重要：你必须直接输出完整的审查文本，不要只报告你读了哪些文件。"
        ),
        ExchangeKind::FollowUp => format!(
            "你是 Agent B，正在进行多 agent 代码审查辩论。\
请阅读 {conv_filename}，只关注 Agent A 的最后一条回复。\n\n\
禁止总结对话历史！禁止重复已讨论过的内容！\
直接针对 A 最后一条回复中的每一条逐项回应：\n\n\
- A 用新证据反驳了你之前的判断：对照源代码重新评估\n\
- A 接受了你的意见：简要确认即可\n\
- A 提出了新问题：对照实际代码验证\n\
- 你仍然不同意 A 的某条：用具体代码引用解释原因\n\n\
严禁修改任何源代码文件！禁止总结历史！\n\n\
你必须在回复末尾单独写一行结论（格式固定，不可省略）：\n\
  完全同意 A 的立场：结论：同意\n\
  部分同意：结论：部分同意\n\
  存在分歧：结论：不同意"
        ),
    }
}

fn build_apply_instruction(apply_level: u32, en: bool) -> &'static str {
    if en {
        return match apply_level {
            2 => {
                "fix high- and medium-priority agreed issues by editing the relevant source files directly; skip low-priority issues"
            }
            3 => "fix all agreed issues by editing the relevant source files directly",
            _ => {
                "fix high-priority agreed issues by editing the relevant source files directly; skip medium/low-priority issues"
            }
        };
    }

    match apply_level {
        2 => {
            "选择高优先级和中优先级问题，阅读相关的源代码文件并直接修改代码来修复这些问题。低优先级问题暂不处理。"
        }
        3 => "修复所有已达成共识的问题，阅读相关的源代码文件并直接修改代码。",
        _ => {
            "选择所有高优先级问题，阅读相关的源代码文件并直接修改代码来修复。中低优先级问题暂不处理。"
        }
    }
}

fn build_apply_prompt(conv_filename: &str, apply_level: u32, en: bool) -> String {
    let apply_instruction = build_apply_instruction(apply_level, en);
    if en {
        return format!(
            "Consensus has been reached. First read the full review conversation in {conv_filename}, \
then {apply_instruction}."
        );
    }

    format!(
        "双方已达成共识。请先阅读当前目录下的 {conv_filename} 文件了解完整审查对话，\
然后根据讨论中达成一致的改进意见，{apply_instruction}"
    )
}

fn discussion_role_label(exchange_kind: ExchangeKind, en: bool) -> &'static str {
    if en {
        return match exchange_kind {
            ExchangeKind::InitialReview => "Initial Review",
            ExchangeKind::RoundReReview => "Re-Review",
            ExchangeKind::FollowUp => "Follow-Up Discussion",
        };
    }

    match exchange_kind {
        ExchangeKind::InitialReview => "初始审查",
        ExchangeKind::RoundReReview => "重新审查",
        ExchangeKind::FollowUp => "后续讨论",
    }
}

/// The state visible to the dashboard.
#[derive(Debug, Clone)]
pub struct OrchestratorState {
    pub phase: Phase,
    pub current_round: u32,
    pub max_rounds: u32,
    pub current_exchange: u32,
    pub max_exchanges: u32,
    pub no_apply: bool,
    pub apply_level: u32,
    pub agent_a_name: String,
    pub agent_a_timeout_secs: u64,
    pub agent_b_name: String,
    pub agent_b_timeout_secs: u64,
    pub round_durations: Vec<std::time::Duration>,
    pub session_start: Option<Instant>,
    pub logs: Vec<String>,
    pub conversation_preview: Arc<str>,
    pub finished: bool,
    pub error_message: Option<String>,
    pub language: String,
    pub review_branch: Option<String>,
    pub original_branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Init,
    AgentAReviewing,
    AgentBResponding,
    CheckConsensus,
    WaitingForApply,
    ApplyChanges,
    WaitingForMerge,
    Done,
}

const MAX_LOG_LINES: usize = 200;
const PREVIEW_TAIL_LINES: usize = 300;

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Phase::Init => write!(f, "Initializing"),
            Phase::AgentAReviewing => write!(f, "Agent A Reviewing"),
            Phase::AgentBResponding => write!(f, "Agent B Responding"),
            Phase::CheckConsensus => write!(f, "Checking Consensus"),
            Phase::WaitingForApply => write!(f, "Waiting For Apply"),
            Phase::ApplyChanges => write!(f, "Applying Changes"),
            Phase::WaitingForMerge => write!(f, "Waiting For Merge"),
            Phase::Done => write!(f, "Done"),
        }
    }
}

/// Commands sent from the dashboard to the orchestrator.
pub enum OrchestratorCommand {
    ConfirmApply,
    ConfirmMerge,
    Shutdown,
}

fn consume_shutdown_command(
    cmd_rx: &mut Option<mpsc::Receiver<OrchestratorCommand>>,
    state: &mut OrchestratorState,
    state_tx: &watch::Sender<OrchestratorState>,
) -> bool {
    if let Some(rx) = cmd_rx.as_mut() {
        loop {
            match rx.try_recv() {
                Ok(OrchestratorCommand::Shutdown) => {
                    state.phase = Phase::Done;
                    state.finished = true;
                    state.log(if state.is_en() {
                        "Shutdown received, ending session"
                    } else {
                        "收到停止信号，提前结束本次会话"
                    });
                    let _ = state_tx.send(state.clone());
                    return true;
                }
                Ok(OrchestratorCommand::ConfirmApply) | Ok(OrchestratorCommand::ConfirmMerge) => {
                    // Ignore stale confirm commands outside the relevant phase.
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
    }
    false
}

fn expose_branch_info(
    state: &mut OrchestratorState,
    review_branch: &Option<String>,
    original_branch: &Option<String>,
) {
    if let (Some(rb), Some(ob)) = (review_branch, original_branch) {
        state.review_branch = Some(rb.clone());
        state.original_branch = Some(ob.clone());
    }
}

fn finalize_session_state(
    state: &mut OrchestratorState,
    conversation: &Conversation,
    state_tx: &watch::Sender<OrchestratorState>,
) {
    state.phase = Phase::Done;
    state.finished = true;
    state.update_preview(conversation);
    let _ = state_tx.send(state.clone());
}

fn finalize_session_error_state(
    state: &mut OrchestratorState,
    conversation: &Conversation,
    state_tx: &watch::Sender<OrchestratorState>,
    err: &anyhow::Error,
) {
    state.error_message = Some(err.to_string());
    finalize_session_state(state, conversation, state_tx);
}

macro_rules! i18n {
    ($state:expr, $en:expr, $zh:expr) => {{
        let msg: String = if $state.is_en() {
            ($en).into()
        } else {
            ($zh).into()
        };
        msg
    }};
}

enum ExchangeOutcome {
    Continue,
    ConsensusReached,
    ShutdownRequested,
    ExecutionError(anyhow::Error),
}

enum ApplyPhaseOutcome {
    Applied,
    Skipped,
    ShutdownRequested,
}

async fn run_exchange(
    round: u32,
    exchange: u32,
    config: &MdtalkConfig,
    previous_round_code_modified: bool,
    conv_filename: &str,
    project_path: &Path,
    agent_a: &AgentRunner,
    agent_b: &AgentRunner,
    conversation: &Conversation,
    state: &mut OrchestratorState,
    state_tx: &watch::Sender<OrchestratorState>,
    cmd_rx: &mut Option<mpsc::Receiver<OrchestratorCommand>>,
) -> Result<ExchangeOutcome> {
    state.current_exchange = exchange;
    let exchange_kind = classify_exchange(round, exchange);

    if consume_shutdown_command(cmd_rx, state, state_tx) {
        return Ok(ExchangeOutcome::ShutdownRequested);
    }

    if should_append_round_header(exchange) {
        conversation.append_round_header_with_language(round, state.is_en())?;
    }

    state.phase = Phase::AgentAReviewing;
    state.log(&i18n!(
        state,
        format!("R{round} E{exchange}: Agent A ({}) reviewing", agent_a.name),
        format!(
            "第{round}轮 讨论{exchange}: Agent A ({}) 开始审查",
            agent_a.name
        )
    ));
    let _ = state_tx.send(state.clone());

    let a_prompt = build_agent_a_prompt(
        exchange_kind,
        conv_filename,
        previous_round_code_modified,
        state.is_en(),
    );
    let a_label = i18n!(
        state,
        format!(
            "Round {round} Exchange {exchange}: Agent A ({})",
            agent_a.name
        ),
        format!("第{round}轮 讨论{exchange}: Agent A ({})", agent_a.name)
    );
    let last_a_response = match run_agent_with_heartbeat(
        agent_a,
        &a_prompt,
        project_path,
        AgentRunMode::Discussion,
        &a_label,
        state,
        state_tx,
    )
    .await
    {
        Ok(output) => {
            let response = output.content;
            let label = discussion_role_label(exchange_kind, state.is_en());
            conversation.append_agent_entry(&agent_a.name, label, &response)?;
            state.log(&i18n!(
                state,
                format!(
                    "R{round} E{exchange}: Agent A done ({:.0}s)",
                    output.duration.as_secs_f64()
                ),
                format!(
                    "第{round}轮 讨论{exchange}: Agent A 完成 ({:.0}秒)",
                    output.duration.as_secs_f64()
                )
            ));
            response
        }
        Err(e) => {
            if state.is_en() {
                error!("Round {round} Exchange {exchange} Agent A failed: {e}");
            } else {
                error!("第{round}轮 讨论{exchange} Agent A 失败: {e}");
            }
            state.log(&i18n!(
                state,
                format!("R{round} E{exchange}: Agent A failed: {e}"),
                format!("第{round}轮 讨论{exchange}: Agent A 失败: {e}")
            ));
            let _ = state_tx.send(state.clone());
            return Ok(ExchangeOutcome::ExecutionError(anyhow::anyhow!(i18n!(
                state,
                format!("Round {round} Exchange {exchange}: Agent A execution failed: {e}"),
                format!("第{round}轮 讨论{exchange}: Agent A 执行失败: {e}")
            ))));
        }
    };

    state.update_preview(conversation);
    let _ = state_tx.send(state.clone());

    state.phase = Phase::AgentBResponding;
    state.log(&i18n!(
        state,
        format!(
            "R{round} E{exchange}: Agent B ({}) responding",
            agent_b.name
        ),
        format!(
            "第{round}轮 讨论{exchange}: Agent B ({}) 开始回应",
            agent_b.name
        )
    ));
    let _ = state_tx.send(state.clone());

    let b_prompt = build_agent_b_prompt(exchange_kind, conv_filename, state.is_en());
    let b_label = i18n!(
        state,
        format!(
            "Round {round} Exchange {exchange}: Agent B ({})",
            agent_b.name
        ),
        format!("第{round}轮 讨论{exchange}: Agent B ({})", agent_b.name)
    );
    let last_b_response = match run_agent_with_heartbeat(
        agent_b,
        &b_prompt,
        project_path,
        AgentRunMode::Discussion,
        &b_label,
        state,
        state_tx,
    )
    .await
    {
        Ok(output) => {
            let response = output.content;
            conversation.append_agent_entry(
                &agent_b.name,
                if state.is_en() { "Response" } else { "回应" },
                &response,
            )?;
            state.log(&i18n!(
                state,
                format!(
                    "R{round} E{exchange}: Agent B done ({:.0}s)",
                    output.duration.as_secs_f64()
                ),
                format!(
                    "第{round}轮 讨论{exchange}: Agent B 完成 ({:.0}秒)",
                    output.duration.as_secs_f64()
                )
            ));
            response
        }
        Err(e) => {
            if state.is_en() {
                error!("Round {round} Exchange {exchange} Agent B failed: {e}");
            } else {
                error!("第{round}轮 讨论{exchange} Agent B 失败: {e}");
            }
            state.log(&i18n!(
                state,
                format!("R{round} E{exchange}: Agent B failed: {e}"),
                format!("第{round}轮 讨论{exchange}: Agent B 失败: {e}")
            ));
            let _ = state_tx.send(state.clone());
            return Ok(ExchangeOutcome::ExecutionError(anyhow::anyhow!(i18n!(
                state,
                format!("Round {round} Exchange {exchange}: Agent B execution failed: {e}"),
                format!("第{round}轮 讨论{exchange}: Agent B 执行失败: {e}")
            ))));
        }
    };

    state.update_preview(conversation);
    let _ = state_tx.send(state.clone());

    state.phase = Phase::CheckConsensus;
    let _ = state_tx.send(state.clone());

    let is_last = exchange == config.review.max_exchanges;
    let is_first_with_more = exchange == 1 && config.review.max_exchanges > 1;
    let result = if is_first_with_more {
        // Exchange 1 with more rounds available: skip consensus check entirely.
        // B's agreement here only means "A's findings are valid", not "debate is done".
        // Force at least one more exchange so A can respond to B's verification.
        state.log(&i18n!(
            state,
            format!("R{round} E{exchange}: skipping consensus (more exchanges available)"),
            format!("第{round}轮 讨论{exchange}: 跳过共识检测（还有后续讨论）")
        ));
        consensus::ConsensusResult {
            reached: false,
            summary: String::new(),
        }
    } else if is_last {
        consensus::check_b_only(&last_b_response, &config.review.consensus_keywords)
    } else {
        consensus::check_consensus(
            &last_a_response,
            &last_b_response,
            &config.review.consensus_keywords,
        )
    };

    if result.reached {
        state.log(&i18n!(
            state,
            format!("R{round} E{exchange}: consensus reached"),
            format!("第{round}轮 讨论{exchange}: 达成共识")
        ));
        let summary = if state.is_en() {
            "Both agents explicitly expressed consensus via the configured keywords.".to_string()
        } else {
            result.summary.clone()
        };
        conversation.append_consensus_with_language(&summary, state.is_en())?;
        return Ok(ExchangeOutcome::ConsensusReached);
    }

    state.log(&i18n!(
        state,
        format!("R{round} E{exchange}: no consensus, continuing..."),
        format!("第{round}轮 讨论{exchange}: 未达成共识，继续讨论...")
    ));
    let _ = state_tx.send(state.clone());
    Ok(ExchangeOutcome::Continue)
}

async fn run_apply_phase(
    round: u32,
    no_apply: bool,
    auto_apply: bool,
    apply_level: u32,
    conv_filename: &str,
    project_path: &Path,
    agent_b: &AgentRunner,
    conversation: &Conversation,
    state: &mut OrchestratorState,
    state_tx: &watch::Sender<OrchestratorState>,
    cmd_rx: &mut Option<mpsc::Receiver<OrchestratorCommand>>,
) -> Result<ApplyPhaseOutcome> {
    if no_apply {
        state.log(&i18n!(
            state,
            format!("Round {round}: skipping apply (--no-apply)"),
            format!("第{round}轮: 跳过代码修改 (--no-apply)")
        ));
        let _ = state_tx.send(state.clone());
        return Ok(ApplyPhaseOutcome::Skipped);
    }

    if !auto_apply {
        state.phase = Phase::WaitingForApply;
        state.log(&i18n!(
            state,
            format!("Round {round}: waiting for apply confirmation..."),
            format!("第{round}轮: 等待用户确认执行修改...")
        ));
        let _ = state_tx.send(state.clone());

        let mut shutdown_requested = false;
        let confirmed = if let Some(rx) = cmd_rx {
            loop {
                match rx.recv().await {
                    Some(OrchestratorCommand::ConfirmApply) => break true,
                    Some(OrchestratorCommand::Shutdown) => {
                        shutdown_requested = true;
                        break false;
                    }
                    Some(_) => {}
                    None => {
                        shutdown_requested = true;
                        break false;
                    }
                }
            }
        } else {
            true
        };

        if shutdown_requested {
            state.log(&i18n!(
                state,
                "Shutdown received, ending session",
                "收到停止信号，提前结束本次会话"
            ));
            return Ok(ApplyPhaseOutcome::ShutdownRequested);
        }

        if !confirmed {
            state.log(&i18n!(
                state,
                format!("Round {round}: user cancelled apply"),
                format!("第{round}轮: 用户取消修改")
            ));
            let _ = state_tx.send(state.clone());
            return Ok(ApplyPhaseOutcome::Skipped);
        }

        state.log(&i18n!(
            state,
            format!("Round {round}: user confirmed, applying..."),
            format!("第{round}轮: 用户已确认，开始修改...")
        ));
    }

    if consume_shutdown_command(cmd_rx, state, state_tx) {
        return Ok(ApplyPhaseOutcome::ShutdownRequested);
    }

    state.phase = Phase::ApplyChanges;
    state.log(&i18n!(
        state,
        format!("Round {round}: Agent B applying changes..."),
        format!("第{round}轮: Agent B 开始根据共识修改代码...")
    ));
    let _ = state_tx.send(state.clone());

    let apply_prompt = build_apply_prompt(conv_filename, apply_level, state.is_en());
    let apply_label = i18n!(
        state,
        format!("Round {round} Code Changes: Agent B ({})", agent_b.name),
        format!("第{round}轮 代码修改: Agent B ({})", agent_b.name)
    );
    let output = run_agent_with_heartbeat(
        agent_b,
        &apply_prompt,
        project_path,
        AgentRunMode::Apply,
        &apply_label,
        state,
        state_tx,
    )
    .await
    .map_err(|e| {
        anyhow::anyhow!(i18n!(
            state,
            format!("Round {round}: Agent B apply failed: {e}"),
            format!("第{round}轮: Agent B 修改代码失败: {e}")
        ))
    })?;

    conversation.append_agent_entry(
        &agent_b.name,
        if state.is_en() {
            "Code Changes"
        } else {
            "代码修改"
        },
        &output.content,
    )?;

    if let Err(e) = crate::conversation::append_changelog_with_language(
        project_path,
        round,
        &output.content,
        state.is_en(),
    ) {
        state.log(&i18n!(
            state,
            format!("Failed to write review_changelog.md: {e}"),
            format!("写入 review_changelog.md 失败: {e}")
        ));
        anyhow::bail!(
            "{}",
            i18n!(
                state,
                format!("Round {round}: failed to write review_changelog.md: {e}"),
                format!("第{round}轮: 写入 review_changelog.md 失败: {e}")
            )
        );
    }

    state.log(&i18n!(
        state,
        "review_changelog.md updated",
        "review_changelog.md 已更新"
    ));
    state.log(&i18n!(
        state,
        format!(
            "Round {round}: Agent B apply done ({:.0}s)",
            output.duration.as_secs_f64()
        ),
        format!(
            "第{round}轮: Agent B 已完成代码修改 ({:.0}秒)",
            output.duration.as_secs_f64()
        )
    ));

    state.update_preview(conversation);
    let _ = state_tx.send(state.clone());

    Ok(ApplyPhaseOutcome::Applied)
}

async fn run_branch_finalization(
    project_path: &Path,
    review_branch: &Option<String>,
    original_branch: &Option<String>,
    initial_dirty_paths: &HashSet<String>,
    cmd_rx: &mut Option<mpsc::Receiver<OrchestratorCommand>>,
    conversation: &Conversation,
    state: &mut OrchestratorState,
    state_tx: &watch::Sender<OrchestratorState>,
) -> Result<()> {
    let (Some(rb), Some(ob)) = (review_branch, original_branch) else {
        return Ok(());
    };

    state.review_branch = Some(rb.clone());
    state.original_branch = Some(ob.clone());

    let commit_outcome = git_commit_filtered(
        project_path,
        &format!("mdtalk: review changes on {rb}"),
        initial_dirty_paths,
    )
    .await
    .map_err(|e| {
        state.log(&i18n!(
            state,
            format!("Failed to commit changes: {e}"),
            format!("提交更改失败: {e}")
        ));
        let _ = state_tx.send(state.clone());
        anyhow::anyhow!(i18n!(
            state,
            format!("Branch mode: auto-commit failed: {e}"),
            format!("分支模式: 自动提交失败: {e}")
        ))
    })?;

    match commit_outcome {
        GitCommitOutcome::Committed => {
            state.log(&i18n!(
                state,
                format!("Changes committed on branch {rb}"),
                format!("更改已提交到分支 {rb}")
            ));

            state.phase = Phase::WaitingForMerge;
            state.log(&i18n!(
                state,
                "Press Enter to merge, or q to keep branch and exit",
                "按 Enter 合并分支，或按 q 保留分支并退出"
            ));
            state.update_preview(conversation);
            let _ = state_tx.send(state.clone());

            let mut do_merge = false;
            if let Some(rx) = cmd_rx {
                loop {
                    match rx.recv().await {
                        Some(OrchestratorCommand::ConfirmMerge) => {
                            do_merge = true;
                            break;
                        }
                        Some(OrchestratorCommand::Shutdown) => break,
                        Some(_) => {}
                        None => break,
                    }
                }
            }

            if do_merge {
                state.log(&i18n!(state, "Merging...", "正在合并..."));
                let _ = state_tx.send(state.clone());
                match git_checkout_and_merge(project_path, ob, rb).await {
                    Ok(()) => {
                        state.log(&i18n!(
                            state,
                            format!("Merged {rb} into {ob}"),
                            format!("已将 {rb} 合并到 {ob}")
                        ));
                        state.review_branch = None;
                        state.original_branch = None;
                    }
                    Err(e) => {
                        state.log(&i18n!(
                            state,
                            format!("Merge failed: {e}"),
                            format!("合并失败: {e}")
                        ));
                    }
                }
            }
        }
        GitCommitOutcome::NothingToCommit => {
            state.log(&i18n!(
                state,
                "No file changes to commit on review branch",
                "审查分支无可提交的文件变更"
            ));
        }
    }

    Ok(())
}

impl OrchestratorState {
    pub fn new(config: &MdtalkConfig) -> Self {
        Self {
            phase: Phase::Init,
            current_round: 0,
            max_rounds: config.review.max_rounds,
            current_exchange: 0,
            max_exchanges: config.review.max_exchanges,
            no_apply: false,
            apply_level: 1,
            agent_a_name: config.agent_a.name.clone(),
            agent_a_timeout_secs: config.agent_a.timeout_secs,
            agent_b_name: config.agent_b.name.clone(),
            agent_b_timeout_secs: config.agent_b.timeout_secs,
            round_durations: Vec::new(),
            session_start: None,
            logs: Vec::new(),
            conversation_preview: Arc::<str>::from(""),
            finished: false,
            error_message: None,
            language: "zh".to_string(),
            review_branch: None,
            original_branch: None,
        }
    }

    fn is_en(&self) -> bool {
        self.language == "en"
    }

    fn log(&mut self, msg: &str) {
        let now = chrono::Local::now().format("%H:%M:%S");
        self.logs.push(format!("[{now}] {msg}"));
        if self.logs.len() > MAX_LOG_LINES {
            let to_drop = self.logs.len() - MAX_LOG_LINES;
            self.logs.drain(0..to_drop);
        }
    }

    fn update_preview(&mut self, conversation: &Conversation) {
        if let Ok(tail) = conversation.read_tail_lines(PREVIEW_TAIL_LINES) {
            self.conversation_preview = Arc::<str>::from(tail);
        }
    }
}

pub async fn run(
    mut config: MdtalkConfig,
    state_tx: watch::Sender<OrchestratorState>,
    cli_no_apply: bool,
    cli_apply_level: u32,
    start_rx: Option<tokio::sync::oneshot::Receiver<crate::config::StartConfig>>,
    cmd_rx: Option<mpsc::Receiver<OrchestratorCommand>>,
) -> Result<()> {
    let mut state = OrchestratorState::new(&config);
    let mut cmd_rx = cmd_rx;
    info!("编排器已启动");

    // Whether the user wants manual apply confirmation
    let mut no_apply = cli_no_apply;
    let mut auto_apply = true;
    let mut apply_level: u32 = cli_apply_level;
    let mut branch_mode = false;
    let mut original_branch: Option<String> = None;
    let mut review_branch: Option<String> = None;
    let mut initial_dirty_paths: HashSet<String> = HashSet::new();

    // Wait for dashboard confirmation if a start signal receiver is provided
    if let Some(rx) = start_rx {
        info!("等待用户确认开始...");
        match rx.await {
            Ok(sc) => {
                info!("收到开始信号");
                no_apply = no_apply || sc.no_apply;
                auto_apply = sc.auto_apply;
                apply_level = sc.apply_level;
                branch_mode = sc.branch_mode;
                let lang = sc.language.clone();
                config.apply_start_config(sc);
                // Re-initialize state from updated config
                state = OrchestratorState::new(&config);
                state.language = lang;
                state.no_apply = no_apply;
                state.apply_level = apply_level;
                let _ = state_tx.send(state.clone());
            }
            Err(_) => {
                info!("开始信号发送端已关闭，退出");
                return Ok(());
            }
        }
    }

    state.session_start = Some(Instant::now());
    state.no_apply = no_apply;
    state.apply_level = apply_level;
    state.log(if state.is_en() {
        "MDTalk session started"
    } else {
        "MDTalk 会话启动"
    });
    let _ = state_tx.send(state.clone());

    let project_path: PathBuf = config.project.path.clone();
    let project_name = project_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());

    // Validate project path
    if !project_path.is_dir() {
        anyhow::bail!("项目路径 {:?} 不存在或不是目录", project_path);
    }

    // Branch mode: create isolated review branch before writing any session files.
    if branch_mode {
        match git_current_branch(&project_path).await {
            Some(branch) => {
                original_branch = Some(branch.clone());
                let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let new_branch = format!("mdtalk/review-{ts}");
                git_checkout_new_branch(&project_path, &new_branch).await?;
                review_branch = Some(new_branch.clone());
                state.log(&if state.is_en() {
                    format!("Branch mode: created branch {new_branch}")
                } else {
                    format!("分支模式: 已创建分支 {new_branch}")
                });
                match git_status_paths(&project_path).await {
                    Ok(paths) => {
                        initial_dirty_paths = paths;
                    }
                    Err(status_err) => {
                        let rollback_result = async {
                            git_checkout_branch(&project_path, &branch).await?;
                            git_delete_branch(&project_path, &new_branch).await?;
                            Ok::<(), anyhow::Error>(())
                        }
                        .await;
                        match rollback_result {
                            Ok(()) => {
                                anyhow::bail!(
                                    "{}",
                                    if state.is_en() {
                                        format!(
                                            "Branch mode: failed to collect initial git status after creating review branch {new_branch}: {status_err}. Rolled back to {branch}."
                                        )
                                    } else {
                                        format!(
                                            "分支模式: 创建审查分支 {new_branch} 后获取初始 git 状态失败: {status_err}。已回滚到 {branch}。"
                                        )
                                    }
                                );
                            }
                            Err(rollback_err) => {
                                anyhow::bail!(
                                    "{}",
                                    if state.is_en() {
                                        format!(
                                            "Branch mode: failed to collect initial git status after creating review branch {new_branch}: {status_err}. Rollback also failed: {rollback_err}"
                                        )
                                    } else {
                                        format!(
                                            "分支模式: 创建审查分支 {new_branch} 后获取初始 git 状态失败: {status_err}。回滚也失败: {rollback_err}"
                                        )
                                    }
                                );
                            }
                        }
                    }
                }
                let _ = state_tx.send(state.clone());
            }
            None => {
                anyhow::bail!(
                    "{}",
                    if state.is_en() {
                        "Branch mode: current directory is not a git repository; cannot create isolated branch"
                    } else {
                        "分支模式: 当前目录不是 git 仓库，无法创建隔离分支"
                    }
                );
            }
        }
    }

    // Create conversation file in the project directory
    let conversation = Conversation::new(&project_path, &config.review.output_file, &project_name);
    conversation.create_with_language(state.is_en())?;

    let agent_a = AgentRunner::new(&config.agent_a);
    let agent_b = AgentRunner::new(&config.agent_b);
    let conv_filename = config.review.output_file.clone();

    let mut previous_round_code_modified = false;

    // === Outer loop: rounds (each round = discussion → consensus → code fix) ===
    for round in 1..=config.review.max_rounds {
        state.current_round = round;
        state.log(&i18n!(
            state,
            format!(
                "===== Round {round} started ({} total) =====",
                config.review.max_rounds
            ),
            format!(
                "===== 第{round}轮审查开始 (共{}轮) =====",
                config.review.max_rounds
            )
        ));
        let _ = state_tx.send(state.clone());

        if consume_shutdown_command(&mut cmd_rx, &mut state, &state_tx) {
            expose_branch_info(&mut state, &review_branch, &original_branch);
            finalize_session_state(&mut state, &conversation, &state_tx);
            return Ok(());
        }

        let round_start = Instant::now();
        let mut consensus_reached = false;
        let mut execution_error: Option<anyhow::Error> = None;

        // === Inner loop: exchanges (A speaks + B speaks + consensus check) ===
        for exchange in 1..=config.review.max_exchanges {
            let exchange_outcome = match run_exchange(
                round,
                exchange,
                &config,
                previous_round_code_modified,
                &conv_filename,
                &project_path,
                &agent_a,
                &agent_b,
                &conversation,
                &mut state,
                &state_tx,
                &mut cmd_rx,
            )
            .await
            {
                Ok(outcome) => outcome,
                Err(err) => {
                    execution_error = Some(err);
                    break;
                }
            };

            match exchange_outcome {
                ExchangeOutcome::Continue => {}
                ExchangeOutcome::ConsensusReached => {
                    consensus_reached = true;
                    break;
                }
                ExchangeOutcome::ShutdownRequested => {
                    expose_branch_info(&mut state, &review_branch, &original_branch);
                    finalize_session_state(&mut state, &conversation, &state_tx);
                    return Ok(());
                }
                ExchangeOutcome::ExecutionError(err) => {
                    execution_error = Some(err);
                    break;
                }
            }
        }

        state.round_durations.push(round_start.elapsed());

        if let Some(err) = execution_error {
            expose_branch_info(&mut state, &review_branch, &original_branch);
            finalize_session_error_state(&mut state, &conversation, &state_tx, &err);
            return Err(err);
        }

        if !consensus_reached {
            // This round failed to reach consensus
            state.phase = Phase::Done;
            state.finished = true;
            state.log(&i18n!(
                state,
                format!(
                    "Round {round}: no consensus after {} exchanges, review ended",
                    config.review.max_exchanges
                ),
                format!(
                    "第{round}轮: {}次讨论后仍未达成共识，审查结束",
                    config.review.max_exchanges
                )
            ));
            expose_branch_info(&mut state, &review_branch, &original_branch);
            finalize_session_state(&mut state, &conversation, &state_tx);
            if state.is_en() {
                info!("Round {round}: review ended without consensus");
            } else {
                info!("第{round}轮审查未能达成共识");
            }
            return Ok(());
        }

        // === Consensus reached — apply changes ===
        let apply_outcome = match run_apply_phase(
            round,
            no_apply,
            auto_apply,
            apply_level,
            &conv_filename,
            &project_path,
            &agent_b,
            &conversation,
            &mut state,
            &state_tx,
            &mut cmd_rx,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(err) => {
                expose_branch_info(&mut state, &review_branch, &original_branch);
                finalize_session_error_state(&mut state, &conversation, &state_tx, &err);
                return Err(err);
            }
        };

        let code_modified_in_round = match apply_outcome {
            ApplyPhaseOutcome::Applied => true,
            ApplyPhaseOutcome::Skipped => false,
            ApplyPhaseOutcome::ShutdownRequested => {
                expose_branch_info(&mut state, &review_branch, &original_branch);
                finalize_session_state(&mut state, &conversation, &state_tx);
                return Ok(());
            }
        };

        previous_round_code_modified = code_modified_in_round;

        // Check if this was the last round
        if round == config.review.max_rounds {
            state.log(&i18n!(
                state,
                format!("All {} rounds completed", config.review.max_rounds),
                format!("已完成全部{}轮审查", config.review.max_rounds)
            ));
        } else {
            state.log(&i18n!(
                state,
                format!("Round {round} done, next round..."),
                format!("第{round}轮完成，进入下一轮...")
            ));
            let _ = state_tx.send(state.clone());
        }
    }

    // Branch mode: commit changes and optionally wait for merge decision.
    if let Err(err) = run_branch_finalization(
        &project_path,
        &review_branch,
        &original_branch,
        &initial_dirty_paths,
        &mut cmd_rx,
        &conversation,
        &mut state,
        &state_tx,
    )
    .await
    {
        finalize_session_error_state(&mut state, &conversation, &state_tx, &err);
        return Err(err);
    }

    finalize_session_state(&mut state, &conversation, &state_tx);
    if state.is_en() {
        info!(
            "Review session completed ({} rounds)",
            config.review.max_rounds
        );
    } else {
        info!("审查会话完成 (共{}轮)", config.review.max_rounds);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command as StdCommand;

    use tokio::sync::watch;

    use super::{
        ExchangeKind, OrchestratorState, Phase, build_agent_a_prompt, build_agent_b_prompt,
        build_apply_prompt, classify_exchange, decode_git_status_path, git_checkout_and_merge,
        git_commit_filtered, run, run_branch_finalization, should_append_round_header,
    };
    use crate::config::{
        AgentConfig, DashboardConfig, MdtalkConfig, ProjectConfig, ReviewConfig, StartConfig,
    };
    use crate::conversation::Conversation;
    use crate::test_utils::TestTempDir;

    fn git_must_succeed(dir: &Path, args: &[&str]) {
        let output = StdCommand::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("failed to run git command");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    fn write_script(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(format!("{name}.cmd"));
        let content = format!("@echo off\r\n{body}\r\n");
        fs::write(&path, content).expect("failed to write windows script");
        path.to_string_lossy().into_owned()
    }

    #[cfg(unix)]
    fn write_script(dir: &Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join(name);
        let content = format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n");
        fs::write(&path, content).expect("failed to write unix script");
        let mut perms = fs::metadata(&path)
            .expect("failed to stat unix script")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("failed to chmod unix script");
        path.to_string_lossy().into_owned()
    }

    fn test_config(
        project_path: PathBuf,
        agent_a_cmd: String,
        agent_b_cmd: String,
    ) -> MdtalkConfig {
        MdtalkConfig {
            project: ProjectConfig { path: project_path },
            agent_a: AgentConfig {
                name: "agent-a".to_string(),
                command: agent_a_cmd,
                timeout_secs: 10,
            },
            agent_b: AgentConfig {
                name: "agent-b".to_string(),
                command: agent_b_cmd,
                timeout_secs: 10,
            },
            review: ReviewConfig {
                max_rounds: 1,
                max_exchanges: 1,
                consensus_keywords: vec!["I agree".to_string()],
                output_file: "conversation.md".to_string(),
            },
            dashboard: DashboardConfig {
                refresh_rate_ms: 100,
            },
        }
    }

    fn start_config(
        agent_a_cmd: String,
        agent_b_cmd: String,
        branch_mode: bool,
        no_apply: bool,
    ) -> StartConfig {
        StartConfig {
            agent_a_command: agent_a_cmd,
            agent_b_command: agent_b_cmd,
            agent_a_timeout_secs: 10,
            agent_b_timeout_secs: 10,
            max_rounds: 1,
            max_exchanges: 1,
            no_apply,
            auto_apply: true,
            apply_level: 1,
            language: "zh".to_string(),
            branch_mode,
        }
    }

    fn script_always_fail(dir: &Path, name: &str) -> String {
        #[cfg(windows)]
        {
            write_script(dir, name, "echo failed 1>&2\r\nexit /b 1")
        }
        #[cfg(unix)]
        {
            write_script(dir, name, "echo failed >&2\nexit 1")
        }
    }

    fn script_always_agree(dir: &Path, name: &str) -> String {
        #[cfg(windows)]
        {
            write_script(dir, name, "echo I agree\r\nexit /b 0")
        }
        #[cfg(unix)]
        {
            write_script(dir, name, "echo \"I agree\"\nexit 0")
        }
    }

    fn script_never_agree(dir: &Path, name: &str) -> String {
        #[cfg(windows)]
        {
            write_script(dir, name, "echo still reviewing\r\nexit /b 0")
        }
        #[cfg(unix)]
        {
            write_script(dir, name, "echo \"still reviewing\"\nexit 0")
        }
    }

    fn script_fail_on_apply_prompt(dir: &Path, name: &str) -> String {
        #[cfg(windows)]
        {
            let body = format!(
                "set \"MARKER=%~dp0{name}.marker\"\r\nif exist \"%MARKER%\" (\r\n  del \"%MARKER%\" >nul 2>&1\r\n  echo apply failed 1>&2\r\n  exit /b 1\r\n)\r\ntype nul > \"%MARKER%\"\r\necho I agree\r\nexit /b 0"
            );
            write_script(dir, name, &body)
        }
        #[cfg(unix)]
        {
            let body = format!(
                "MARKER=\"$(dirname \"$0\")/{name}.marker\"\nif [ -f \"$MARKER\" ]; then\n  rm -f \"$MARKER\"\n  echo \"apply failed\" >&2\n  exit 1\nfi\n: > \"$MARKER\"\necho \"I agree\"\nexit 0"
            );
            write_script(dir, name, &body)
        }
    }

    fn script_break_conversation_file(dir: &Path, name: &str) -> String {
        #[cfg(windows)]
        {
            write_script(
                dir,
                name,
                "if exist conversation.md del /f /q conversation.md >nul 2>&1\r\nmkdir conversation.md >nul 2>&1\r\necho I agree\r\nexit /b 0",
            )
        }
        #[cfg(unix)]
        {
            write_script(
                dir,
                name,
                "rm -f conversation.md\nmkdir -p conversation.md\necho \"I agree\"\nexit 0",
            )
        }
    }

    #[test]
    fn first_exchange_in_first_round_is_initial_review() {
        assert_eq!(classify_exchange(1, 1), ExchangeKind::InitialReview);
    }

    #[test]
    fn first_exchange_in_later_round_is_rereview() {
        assert_eq!(classify_exchange(2, 1), ExchangeKind::RoundReReview);
    }

    #[test]
    fn round_header_is_only_written_once_per_round() {
        assert!(should_append_round_header(1));
        assert!(!should_append_round_header(2));
        assert!(!should_append_round_header(3));
    }

    #[test]
    fn english_agent_a_prompt_is_localized() {
        let prompt =
            build_agent_a_prompt(ExchangeKind::InitialReview, "conversation.md", true, true);
        assert!(prompt.contains("You are participating in a multi-agent code review workflow"));
        assert!(!prompt.contains("你正在参与"));
    }

    #[test]
    fn rereview_prompt_mentions_modified_code_when_apply_ran() {
        let prompt =
            build_agent_a_prompt(ExchangeKind::RoundReReview, "conversation.md", true, true);
        assert!(prompt.contains("Code was modified after the previous round."));
        assert!(!prompt.contains("No code changes were applied after the previous round."));
    }

    #[test]
    fn rereview_prompt_mentions_skipped_apply_when_no_code_change() {
        let prompt =
            build_agent_a_prompt(ExchangeKind::RoundReReview, "conversation.md", false, true);
        assert!(prompt.contains("No code changes were applied after the previous round."));
        assert!(!prompt.contains("Code was modified after the previous round."));
    }

    #[test]
    fn english_agent_b_prompt_is_localized() {
        let prompt = build_agent_b_prompt(ExchangeKind::InitialReview, "conversation.md", true);
        assert!(prompt.contains("You are an independent code review expert"));
        assert!(!prompt.contains("你是一位独立的代码审查专家"));
    }

    #[test]
    fn followup_agent_b_prompt_mentions_discussion_flow() {
        let prompt = build_agent_b_prompt(ExchangeKind::FollowUp, "conversation.md", true);
        assert!(prompt.contains("Agent A's LAST response"));
        assert!(!prompt.contains("Verify each finding"));
    }

    #[test]
    fn english_agent_b_prompt_has_no_indented_step_prefix() {
        let prompt = build_agent_b_prompt(ExchangeKind::InitialReview, "conversation.md", true);
        assert!(
            prompt.contains("\nSteps:\n"),
            "english prompt should expose an unindented Steps header"
        );
        assert!(
            !prompt.contains("\n                Steps:\n"),
            "english prompt should not contain source indentation spaces in content"
        );
    }

    #[test]
    fn chinese_agent_b_prompt_has_no_indented_step_prefix() {
        let prompt = build_agent_b_prompt(ExchangeKind::InitialReview, "conversation.md", false);
        assert!(
            prompt.contains("\n具体步骤：\n"),
            "chinese prompt should expose an unindented 具体步骤 header"
        );
        assert!(
            !prompt.contains("\n            具体步骤：\n"),
            "chinese prompt should not contain source indentation spaces in content"
        );
    }

    #[test]
    fn english_apply_prompt_is_localized() {
        let prompt = build_apply_prompt("conversation.md", 1, true);
        assert!(prompt.contains("Consensus has been reached"));
        assert!(!prompt.contains("双方已达成共识"));
    }

    #[tokio::test]
    async fn returns_err_when_agent_a_discussion_fails() {
        let temp_dir = TestTempDir::new("orchestrator", "agent-a-fails");
        let project_dir = temp_dir.path().to_path_buf();
        let fail_cmd = script_always_fail(&project_dir, "agent_a_fail");
        let ok_cmd = script_always_agree(&project_dir, "agent_b_ok");
        let cfg = test_config(project_dir.clone(), fail_cmd, ok_cmd);
        let (state_tx, state_rx) = watch::channel(OrchestratorState::new(&cfg));

        let result = run(cfg, state_tx, true, 1, None, None).await;
        assert!(
            result.is_err(),
            "Agent A execution failure should return Err"
        );

        let final_state = state_rx.borrow().clone();
        assert_eq!(final_state.phase, Phase::Done);
        assert!(
            final_state.error_message.is_some(),
            "dashboard state should expose execution failure context"
        );
    }

    #[tokio::test]
    async fn returns_err_when_apply_phase_fails() {
        let temp_dir = TestTempDir::new("orchestrator", "apply-fails");
        let project_dir = temp_dir.path().to_path_buf();
        let a_ok_cmd = script_always_agree(&project_dir, "agent_a_ok");
        let b_cmd = script_fail_on_apply_prompt(&project_dir, "agent_b_fail_on_second_call");
        let cfg = test_config(project_dir.clone(), a_ok_cmd, b_cmd);
        let (state_tx, _state_rx) = watch::channel(OrchestratorState::new(&cfg));
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let mut sc = start_config(
            cfg.agent_a.command.clone(),
            cfg.agent_b.command.clone(),
            false,
            false,
        );
        sc.language = "en".to_string();
        start_tx.send(sc).expect("failed to send start config");

        let result = run(cfg, state_tx, false, 1, Some(start_rx), None).await;
        assert!(result.is_err(), "Apply-phase failure should return Err");
    }

    #[tokio::test]
    async fn finalizes_session_error_state_when_conversation_write_fails_in_exchange() {
        let temp_dir = TestTempDir::new("orchestrator", "conversation-write-fails");
        let project_dir = temp_dir.path().to_path_buf();
        let a_cmd = script_break_conversation_file(&project_dir, "agent_a_breaks_conversation");
        let b_cmd = script_always_agree(&project_dir, "agent_b_ok");
        let cfg = test_config(project_dir, a_cmd, b_cmd);
        let (state_tx, state_rx) = watch::channel(OrchestratorState::new(&cfg));

        let result = run(cfg, state_tx, true, 1, None, None).await;
        assert!(
            result.is_err(),
            "conversation append failure should surface as an error"
        );

        let final_state = state_rx.borrow().clone();
        assert_eq!(final_state.phase, Phase::Done);
        assert!(
            final_state.error_message.is_some(),
            "finalized state should expose the write failure to dashboard"
        );
    }

    #[tokio::test]
    async fn closed_apply_confirmation_channel_does_not_auto_apply() {
        let temp_dir = TestTempDir::new("orchestrator", "apply-channel-closed");
        let project_dir = temp_dir.path().to_path_buf();
        let a_ok_cmd = script_always_agree(&project_dir, "agent_a_ok");
        let b_cmd = script_fail_on_apply_prompt(&project_dir, "agent_b_fail_on_apply");
        let cfg = test_config(project_dir.clone(), a_ok_cmd.clone(), b_cmd.clone());
        let (state_tx, state_rx) = watch::channel(OrchestratorState::new(&cfg));
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let mut sc = start_config(a_ok_cmd, b_cmd, false, false);
        sc.auto_apply = false;
        start_tx.send(sc).expect("failed to send start config");

        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel(1);
        drop(cmd_tx);

        let result = run(cfg, state_tx, false, 1, Some(start_rx), Some(cmd_rx)).await;
        assert!(
            result.is_ok(),
            "closed command channel should not be treated as apply confirmation"
        );

        let final_state = state_rx.borrow().clone();
        assert_eq!(final_state.phase, Phase::Done);
        assert!(
            final_state.error_message.is_none(),
            "channel close should end safely without apply failure"
        );
    }

    #[tokio::test]
    async fn start_config_no_apply_skips_apply_phase() {
        let temp_dir = TestTempDir::new("orchestrator", "start-config-no-apply");
        let project_dir = temp_dir.path().to_path_buf();
        let a_ok_cmd = script_always_agree(&project_dir, "agent_a_ok");
        let b_cmd = script_fail_on_apply_prompt(&project_dir, "agent_b_fail_on_apply_prompt");
        let cfg = test_config(project_dir.clone(), a_ok_cmd.clone(), b_cmd.clone());
        let (state_tx, _state_rx) = watch::channel(OrchestratorState::new(&cfg));
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        start_tx
            .send(start_config(a_ok_cmd, b_cmd, false, true))
            .expect("failed to send start config");

        let result = run(cfg, state_tx, false, 1, Some(start_rx), None).await;
        assert!(
            result.is_ok(),
            "start-screen no_apply should skip apply even when CLI no_apply=false, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn start_config_apply_level_is_reflected_in_state() {
        let temp_dir = TestTempDir::new("orchestrator", "start-config-apply-level");
        let project_dir = temp_dir.path().to_path_buf();
        let a_ok_cmd = script_always_agree(&project_dir, "agent_a_ok");
        let b_ok_cmd = script_always_agree(&project_dir, "agent_b_ok");
        let cfg = test_config(project_dir, a_ok_cmd.clone(), b_ok_cmd.clone());
        let (state_tx, state_rx) = watch::channel(OrchestratorState::new(&cfg));
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let mut sc = start_config(a_ok_cmd, b_ok_cmd, false, true);
        sc.apply_level = 3;
        start_tx.send(sc).expect("failed to send start config");

        let result = run(cfg, state_tx, false, 1, Some(start_rx), None).await;
        assert!(
            result.is_ok(),
            "run should succeed for apply-level snapshot"
        );

        let final_state = state_rx.borrow().clone();
        assert_eq!(
            final_state.apply_level, 3,
            "orchestrator state should reflect apply level chosen on start screen"
        );
    }

    #[tokio::test]
    async fn branch_mode_errors_when_review_branch_cannot_be_created() {
        let temp_dir = TestTempDir::new("orchestrator", "branch-mode-non-git");
        let project_dir = temp_dir.path().to_path_buf();
        let a_ok_cmd = script_always_agree(&project_dir, "agent_a_ok");
        let b_ok_cmd = script_always_agree(&project_dir, "agent_b_ok");
        let cfg = test_config(project_dir.clone(), a_ok_cmd.clone(), b_ok_cmd.clone());
        let (state_tx, _state_rx) = watch::channel(OrchestratorState::new(&cfg));
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        start_tx
            .send(start_config(a_ok_cmd, b_ok_cmd, true, false))
            .expect("failed to send start config");

        let result = run(cfg, state_tx, false, 1, Some(start_rx), None).await;
        assert!(
            result.is_err(),
            "Branch mode must fail fast when review branch cannot be created"
        );
        assert!(
            !project_dir.join("conversation.md").exists(),
            "Branch setup should run before creating conversation.md in branch mode"
        );
    }

    #[tokio::test]
    async fn branch_mode_rolls_back_when_initial_status_snapshot_fails() {
        let temp_dir = TestTempDir::new("orchestrator", "branch-mode-status-snapshot-fails");
        let project_dir = temp_dir.path().to_path_buf();
        git_must_succeed(&project_dir, &["init"]);
        git_must_succeed(&project_dir, &["config", "user.email", "test@example.com"]);
        git_must_succeed(&project_dir, &["config", "user.name", "MDTalk Test"]);
        fs::write(project_dir.join("seed.txt"), "base\n").expect("failed to write seed file");
        git_must_succeed(&project_dir, &["add", "seed.txt"]);
        git_must_succeed(&project_dir, &["commit", "-m", "base"]);

        let original_branch_output = StdCommand::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&project_dir)
            .output()
            .expect("failed to read current branch");
        assert!(
            original_branch_output.status.success(),
            "failed to read branch name: {}",
            String::from_utf8_lossy(&original_branch_output.stderr)
        );
        let original_branch = String::from_utf8_lossy(&original_branch_output.stdout)
            .trim()
            .to_string();
        assert!(
            !original_branch.is_empty(),
            "original branch should not be empty"
        );

        // Make `git status` fail while `git checkout -b` still succeeds.
        git_must_succeed(
            &project_dir,
            &["config", "status.showUntrackedFiles", "invalid"],
        );

        let a_ok_cmd = script_always_agree(&project_dir, "agent_a_ok");
        let b_ok_cmd = script_always_agree(&project_dir, "agent_b_ok");
        let cfg = test_config(project_dir.clone(), a_ok_cmd.clone(), b_ok_cmd.clone());
        let (state_tx, _state_rx) = watch::channel(OrchestratorState::new(&cfg));
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        start_tx
            .send(start_config(a_ok_cmd, b_ok_cmd, true, false))
            .expect("failed to send start config");

        let result = run(cfg, state_tx, true, 1, Some(start_rx), None).await;
        assert!(
            result.is_err(),
            "branch-mode setup should fail when initial git status snapshot fails"
        );

        let branch_output = StdCommand::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&project_dir)
            .output()
            .expect("failed to read branch after run");
        assert!(
            branch_output.status.success(),
            "failed to read branch after rollback: {}",
            String::from_utf8_lossy(&branch_output.stderr)
        );
        let current_branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();
        assert_eq!(
            current_branch, original_branch,
            "branch-mode setup failure should restore original branch"
        );

        let review_branches_output = StdCommand::new("git")
            .args(["branch", "--list", "mdtalk/review-*"])
            .current_dir(&project_dir)
            .output()
            .expect("failed to list review branches");
        assert!(
            review_branches_output.status.success(),
            "failed to list review branches: {}",
            String::from_utf8_lossy(&review_branches_output.stderr)
        );
        let review_branches = String::from_utf8_lossy(&review_branches_output.stdout);
        assert!(
            review_branches.trim().is_empty(),
            "review branch should be deleted after setup rollback, got: {review_branches:?}"
        );
    }

    #[tokio::test]
    async fn branch_mode_commit_excludes_preexisting_dirty_files() {
        let temp_dir = TestTempDir::new("orchestrator", "branch-mode-dirty-filter");
        let project_dir = temp_dir.path().to_path_buf();
        git_must_succeed(&project_dir, &["init"]);
        git_must_succeed(&project_dir, &["config", "user.email", "test@example.com"]);
        git_must_succeed(&project_dir, &["config", "user.name", "MDTalk Test"]);

        fs::write(project_dir.join("preexisting.txt"), "base\n")
            .expect("failed to write base file");
        git_must_succeed(&project_dir, &["add", "preexisting.txt"]);
        git_must_succeed(&project_dir, &["commit", "-m", "base"]);

        // Dirty change exists before the orchestrator starts.
        fs::write(
            project_dir.join("preexisting.txt"),
            "dirty-before-session\n",
        )
        .expect("failed to write preexisting dirty file");

        let a_ok_cmd = script_always_agree(&project_dir, "agent_a_ok");
        let b_ok_cmd = script_always_agree(&project_dir, "agent_b_ok");
        let cfg = test_config(project_dir.clone(), a_ok_cmd.clone(), b_ok_cmd.clone());
        let (state_tx, state_rx) = watch::channel(OrchestratorState::new(&cfg));
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        start_tx
            .send(start_config(a_ok_cmd, b_ok_cmd, true, false))
            .expect("failed to send start config");

        let result = run(cfg, state_tx, true, 1, Some(start_rx), None).await;
        assert!(result.is_ok(), "run should succeed in branch mode");

        let final_state = state_rx.borrow().clone();
        assert!(
            final_state.review_branch.is_some() && final_state.original_branch.is_some(),
            "branch mode should keep merge info when no merge command is sent"
        );

        let show_output = StdCommand::new("git")
            .args(["show", "--name-only", "--pretty=format:"])
            .current_dir(&project_dir)
            .output()
            .expect("failed to inspect latest commit");
        assert!(
            show_output.status.success(),
            "git show failed: {}",
            String::from_utf8_lossy(&show_output.stderr)
        );
        let changed_files = String::from_utf8_lossy(&show_output.stdout);
        assert!(
            changed_files
                .lines()
                .any(|line| line.trim() == "conversation.md"),
            "session commit should include conversation.md"
        );
        assert!(
            !changed_files
                .lines()
                .any(|line| line.trim() == "preexisting.txt"),
            "session commit should not include dirty changes that existed before the session"
        );
    }

    #[tokio::test]
    async fn branch_mode_keeps_branch_info_when_no_consensus() {
        let temp_dir = TestTempDir::new("orchestrator", "branch-mode-no-consensus");
        let project_dir = temp_dir.path().to_path_buf();
        git_must_succeed(&project_dir, &["init"]);
        git_must_succeed(&project_dir, &["config", "user.email", "test@example.com"]);
        git_must_succeed(&project_dir, &["config", "user.name", "MDTalk Test"]);

        fs::write(project_dir.join("seed.txt"), "base\n").expect("failed to write seed file");
        git_must_succeed(&project_dir, &["add", "seed.txt"]);
        git_must_succeed(&project_dir, &["commit", "-m", "base"]);

        let a_cmd = script_never_agree(&project_dir, "agent_a_no_consensus");
        let b_cmd = script_never_agree(&project_dir, "agent_b_no_consensus");
        let cfg = test_config(project_dir.clone(), a_cmd.clone(), b_cmd.clone());
        let (state_tx, state_rx) = watch::channel(OrchestratorState::new(&cfg));
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        start_tx
            .send(start_config(a_cmd, b_cmd, true, false))
            .expect("failed to send start config");

        let result = run(cfg, state_tx, true, 1, Some(start_rx), None).await;
        assert!(result.is_ok(), "no-consensus path should return Ok");

        let final_state = state_rx.borrow().clone();
        assert!(
            final_state.review_branch.is_some() && final_state.original_branch.is_some(),
            "branch info should still be exposed when branch mode exits early"
        );
    }

    #[tokio::test]
    async fn git_commit_filtered_returns_error_outside_git_repo() {
        let temp_dir = TestTempDir::new("orchestrator", "git-commit-non-git");
        let project_dir = temp_dir.path().to_path_buf();
        fs::write(project_dir.join("file.txt"), "content").expect("failed to write test file");

        let result = git_commit_filtered(&project_dir, "test commit", &HashSet::new()).await;
        assert!(
            result.is_err(),
            "git_commit_filtered should fail when git status cannot run"
        );
    }

    #[tokio::test]
    async fn git_commit_filtered_keeps_unrelated_staged_entries() {
        let temp_dir = TestTempDir::new("orchestrator", "git-commit-keep-staged");
        let project_dir = temp_dir.path().to_path_buf();
        git_must_succeed(&project_dir, &["init"]);
        git_must_succeed(&project_dir, &["config", "user.email", "test@example.com"]);
        git_must_succeed(&project_dir, &["config", "user.name", "MDTalk Test"]);

        fs::write(project_dir.join("keep_staged.txt"), "base\n")
            .expect("failed to write keep file");
        fs::write(project_dir.join("session.txt"), "base\n").expect("failed to write session file");
        git_must_succeed(&project_dir, &["add", "keep_staged.txt", "session.txt"]);
        git_must_succeed(&project_dir, &["commit", "-m", "base"]);

        // Pre-existing staged change should remain staged after the filtered commit.
        fs::write(project_dir.join("keep_staged.txt"), "preexisting staged\n")
            .expect("failed to modify keep file");
        git_must_succeed(&project_dir, &["add", "keep_staged.txt"]);

        // Session change should be committed.
        fs::write(project_dir.join("session.txt"), "session change\n")
            .expect("failed to modify session file");

        let mut excluded = HashSet::new();
        excluded.insert("keep_staged.txt".to_string());

        let outcome = git_commit_filtered(&project_dir, "session commit", &excluded)
            .await
            .expect("filtered commit should succeed");
        assert_eq!(
            outcome,
            super::GitCommitOutcome::Committed,
            "session file should produce a commit"
        );

        let staged_after = StdCommand::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(&project_dir)
            .output()
            .expect("failed to inspect staged files");
        assert!(
            staged_after.status.success(),
            "git diff --cached failed: {}",
            String::from_utf8_lossy(&staged_after.stderr)
        );
        let staged_files = String::from_utf8_lossy(&staged_after.stdout);
        assert!(
            staged_files
                .lines()
                .any(|line| line.trim() == "keep_staged.txt"),
            "pre-existing staged path should remain staged after filtered commit"
        );

        let show_output = StdCommand::new("git")
            .args(["show", "--name-only", "--pretty=format:"])
            .current_dir(&project_dir)
            .output()
            .expect("failed to inspect latest commit");
        assert!(
            show_output.status.success(),
            "git show failed: {}",
            String::from_utf8_lossy(&show_output.stderr)
        );
        let changed_files = String::from_utf8_lossy(&show_output.stdout);
        assert!(
            changed_files
                .lines()
                .any(|line| line.trim() == "session.txt"),
            "filtered commit should include session.txt"
        );
        assert!(
            !changed_files
                .lines()
                .any(|line| line.trim() == "keep_staged.txt"),
            "filtered commit should exclude pre-existing staged file"
        );
    }

    #[tokio::test]
    async fn git_checkout_and_merge_aborts_conflict_state() {
        let temp_dir = TestTempDir::new("orchestrator", "merge-abort-on-conflict");
        let project_dir = temp_dir.path().to_path_buf();
        git_must_succeed(&project_dir, &["init"]);
        git_must_succeed(&project_dir, &["config", "user.email", "test@example.com"]);
        git_must_succeed(&project_dir, &["config", "user.name", "MDTalk Test"]);

        let tracked = project_dir.join("conflict.txt");
        fs::write(&tracked, "base\n").expect("failed to write base file");
        git_must_succeed(&project_dir, &["add", "conflict.txt"]);
        git_must_succeed(&project_dir, &["commit", "-m", "base"]);

        let branch_output = StdCommand::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&project_dir)
            .output()
            .expect("failed to read current branch");
        assert!(
            branch_output.status.success(),
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&branch_output.stderr)
        );
        let base_branch = String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_string();
        assert!(!base_branch.is_empty(), "base branch should not be empty");

        git_must_succeed(&project_dir, &["checkout", "-b", "feature"]);
        fs::write(&tracked, "feature\n").expect("failed to write feature content");
        git_must_succeed(&project_dir, &["add", "conflict.txt"]);
        git_must_succeed(&project_dir, &["commit", "-m", "feature-change"]);

        git_must_succeed(&project_dir, &["checkout", &base_branch]);
        fs::write(&tracked, "base-branch\n").expect("failed to write base-branch content");
        git_must_succeed(&project_dir, &["add", "conflict.txt"]);
        git_must_succeed(&project_dir, &["commit", "-m", "base-change"]);

        let err = git_checkout_and_merge(&project_dir, &base_branch, "feature")
            .await
            .expect_err("merge conflict should return an error");
        assert!(err.to_string().contains("git merge feature failed"));

        assert!(
            !project_dir.join(".git").join("MERGE_HEAD").exists(),
            "merge conflict should be aborted automatically to clear MERGE_HEAD"
        );
    }

    #[tokio::test]
    async fn branch_finalization_commit_error_is_sent_to_dashboard_immediately() {
        let temp_dir = TestTempDir::new("orchestrator", "branch-finalization-send-on-error");
        let project_dir = temp_dir.path().to_path_buf();
        let cfg = test_config(
            project_dir.clone(),
            "agent_a_cmd".to_string(),
            "agent_b_cmd".to_string(),
        );
        let mut state = OrchestratorState::new(&cfg);
        state.language = "en".to_string();
        let (state_tx, state_rx) = watch::channel(state.clone());
        let mut cmd_rx = None;
        let conversation = Conversation::new(&project_dir, "conversation.md", "project");
        conversation
            .create_with_language(true)
            .expect("failed to create conversation file");

        let err = run_branch_finalization(
            &project_dir,
            &Some("mdtalk/review-test".to_string()),
            &Some("main".to_string()),
            &HashSet::new(),
            &mut cmd_rx,
            &conversation,
            &mut state,
            &state_tx,
        )
        .await
        .expect_err("commit in non-git directory should fail");
        assert!(
            err.to_string().contains("auto-commit failed"),
            "finalization failure should retain context"
        );

        let state_after = state_rx.borrow().clone();
        assert!(
            state_after
                .logs
                .iter()
                .any(|line| line.contains("Failed to commit changes")),
            "commit failure log should be sent before the caller handles the error"
        );
    }

    #[test]
    fn state_logs_are_bounded() {
        let temp_dir = TestTempDir::new("orchestrator", "state-log-cap");
        let project_dir = temp_dir.path().to_path_buf();
        let cfg = test_config(
            project_dir.clone(),
            "agent_a_cmd".to_string(),
            "agent_b_cmd".to_string(),
        );
        let mut state = OrchestratorState::new(&cfg);

        for i in 0..400 {
            state.log(&format!("log line {i}"));
        }

        assert!(
            state.logs.len() <= 200,
            "logs should be capped to keep state cloning bounded"
        );
        assert!(
            state
                .logs
                .last()
                .is_some_and(|line| line.contains("log line 399")),
            "latest logs should be kept"
        );
    }

    #[test]
    fn state_clone_keeps_log_values_independent() {
        let temp_dir = TestTempDir::new("orchestrator", "state-log-arc");
        let project_dir = temp_dir.path().to_path_buf();
        let cfg = test_config(
            project_dir.clone(),
            "agent_a_cmd".to_string(),
            "agent_b_cmd".to_string(),
        );
        let mut state = OrchestratorState::new(&cfg);
        state.log("line 1");
        let mut cloned = state.clone();
        assert_eq!(state.logs.len(), 1, "original state should keep first log");
        assert_eq!(
            cloned.logs.len(),
            1,
            "cloned state should copy initial logs"
        );

        cloned.log("line 2");
        assert!(
            state.logs.iter().all(|line| !line.contains("line 2")),
            "mutating cloned logs should not mutate the original state snapshot"
        );
        assert!(
            cloned.logs.iter().any(|line| line.contains("line 2")),
            "cloned state should accept further log writes"
        );
    }

    #[test]
    fn decode_git_status_path_accepts_single_character_path() {
        let decoded = decode_git_status_path(b"a").expect("single-char path should decode");
        assert_eq!(decoded, "a");
    }

    #[test]
    fn decode_git_status_path_rejects_non_utf8_bytes() {
        let err = decode_git_status_path(&[0x66, 0x6f, 0x80])
            .expect_err("non-UTF-8 path bytes must be rejected");
        assert!(err.to_string().contains("non-UTF-8"));
    }
}
