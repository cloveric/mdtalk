use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::{mpsc, watch};
use tracing::{error, info};

use tokio::process::Command as TokioCommand;

use crate::agent::{AgentOutput, AgentRunner};
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
        if entry.len() < 4 {
            continue;
        }

        let status_x = entry[0] as char;
        let status_y = entry[1] as char;
        let path = String::from_utf8_lossy(&entry[3..]).to_string();
        if !path.is_empty() {
            paths.insert(path);
        }

        // In porcelain -z mode, renames/copies are followed by an extra path token.
        if (matches!(status_x, 'R' | 'C') || matches!(status_y, 'R' | 'C'))
            && let Some(next_path) = entries.next()
        {
            let next = String::from_utf8_lossy(next_path).to_string();
            if !next.is_empty() {
                paths.insert(next);
            }
        }
    }

    Ok(paths)
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
        anyhow::bail!(
            "git merge {merge_branch} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Run an agent while sending heartbeat logs to the dashboard every 30 seconds.
async fn run_agent_with_heartbeat(
    agent: &AgentRunner,
    prompt: &str,
    project_path: &Path,
    label: &str,
    state: &mut OrchestratorState,
    state_tx: &watch::Sender<OrchestratorState>,
) -> Result<AgentOutput> {
    let start = Instant::now();
    let agent_fut = agent.run(prompt, project_path);
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

fn build_agent_a_prompt(exchange_kind: ExchangeKind, conv_filename: &str, en: bool) -> String {
    if en {
        return match exchange_kind {
            ExchangeKind::InitialReview => "You are participating in a multi-agent code review workflow. \
Please thoroughly read the project's source files (src/ directory) and provide a detailed review, including:\n\
- Potential bugs and logic errors\n\
- Code quality issues\n\
- Architecture/design issues\n\
- Improvement suggestions\n\n\
Prioritize findings by severity."
                .to_string(),
            ExchangeKind::RoundReReview => format!(
                "You are participating in a multi-agent code review workflow. \
Code was modified after the previous round. \
First read the full review history in {conv_filename}, then re-review src/ to verify previously identified issues are fixed and to detect any newly introduced issues."
            ),
            ExchangeKind::FollowUp => format!(
                "You are participating in a multi-agent code review workflow. \
First read the full conversation history in {conv_filename}.\n\n\
Then continue the discussion based on Agent B's latest response. \
State whether you agree and provide further thoughts. \
If you fully agree with the other side, explicitly say \"I agree\" or \"consensus reached\"."
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
请按优先级排列你的发现。"
            .to_string(),
        ExchangeKind::RoundReReview => format!(
            "你正在参与一个多 agent 代码审查流程。\
上一轮审查后代码已被修改。\
请先阅读当前目录下的 {conv_filename} 文件了解完整的审查对话历史，\
然后重新审查 src/ 目录下的源代码，检查之前发现的问题是否已修复，\
以及是否引入了新问题。给出你的审查意见。"
        ),
        ExchangeKind::FollowUp => format!(
            "你正在参与一个多 agent 代码审查流程。\
请先阅读当前目录下的 {conv_filename} 文件，了解完整的审查对话历史。\n\n\
然后根据 Agent B 的最新反馈继续讨论。\
表达你是否同意以及你的进一步看法。\
如果你已完全同意对方观点，请明确说 \"I agree\" 或 \"达成一致\"。"
        ),
    }
}

fn build_agent_b_prompt(conv_filename: &str, en: bool) -> String {
    if en {
        return format!(
            "You are an independent code review expert. Verify each finding recorded in '{conv_filename}'.\n\n\
Steps:\n\
1. Read '{conv_filename}' and list all findings from the other reviewer\n\
2. Open the related source files and verify each finding against actual code\n\
3. Output your complete review response directly in this format:\n\
   - Mark each finding as [Agree] or [Disagree], with concrete code evidence\n\
   - Add any missed issues\n\
   - End with a mandatory conclusion line (this line is required and must appear exactly):\n\
     If you agree overall: write exactly → CONCLUSION: I agree\n\
     If you disagree: write exactly → CONCLUSION: I disagree\n\n\
Important: output the full review content directly; do not only report which files you read."
        );
    }

    format!(
        "你是一位独立的代码审查专家。你的任务是对 '{conv_filename}' 中记录的代码审查意见进行逐条验证。\n\n\
具体步骤：\n\
1. 读取 '{conv_filename}' 文件，找到另一位审查者提出的所有发现\n\
2. 对每一条发现，打开对应的源代码文件，核实该问题是否真实存在\n\
3. 直接输出你的完整审查回应，格式如下：\n\
   - 对每条发现标注【同意】或【不同意】，附上你在源代码中看到的证据\n\
   - 补充任何审查者遗漏的新问题\n\
   - 在最后必须单独写一行结论（此行为强制要求，格式固定）：\n\
     如果你整体认可审查意见：写 → 结论：同意\n\
     如果你存在重大分歧：写 → 结论：不同意\n\n\
重要：你必须直接输出完整的审查文本，不要只报告你读了哪些文件。"
    )
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
    pub agent_a_name: String,
    pub agent_a_timeout_secs: u64,
    pub agent_b_name: String,
    pub agent_b_timeout_secs: u64,
    pub round_durations: Vec<std::time::Duration>,
    pub session_start: Option<Instant>,
    pub logs: Vec<String>,
    pub conversation_preview: String,
    pub finished: bool,
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
            Phase::Init => write!(f, "初始化"),
            Phase::AgentAReviewing => write!(f, "Agent A 审查中"),
            Phase::AgentBResponding => write!(f, "Agent B 回应中"),
            Phase::CheckConsensus => write!(f, "检测共识"),
            Phase::WaitingForApply => write!(f, "等待确认修改"),
            Phase::ApplyChanges => write!(f, "修改代码中"),
            Phase::WaitingForMerge => write!(f, "等待合并"),
            Phase::Done => write!(f, "已完成"),
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

impl OrchestratorState {
    pub fn new(config: &MdtalkConfig) -> Self {
        Self {
            phase: Phase::Init,
            current_round: 0,
            max_rounds: config.review.max_rounds,
            current_exchange: 0,
            max_exchanges: config.review.max_exchanges,
            agent_a_name: config.agent_a.name.clone(),
            agent_a_timeout_secs: config.agent_a.timeout_secs,
            agent_b_name: config.agent_b.name.clone(),
            agent_b_timeout_secs: config.agent_b.timeout_secs,
            round_durations: Vec::new(),
            session_start: None,
            logs: Vec::new(),
            conversation_preview: String::new(),
            finished: false,
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
            self.conversation_preview = tail;
        }
    }
}

pub async fn run(
    mut config: MdtalkConfig,
    state_tx: watch::Sender<OrchestratorState>,
    no_apply: bool,
    cli_apply_level: u32,
    start_rx: Option<tokio::sync::oneshot::Receiver<crate::config::StartConfig>>,
    cmd_rx: Option<mpsc::Receiver<OrchestratorCommand>>,
) -> Result<()> {
    let mut state = OrchestratorState::new(&config);
    let mut cmd_rx = cmd_rx;
    info!("编排器已启动");

    // Whether the user wants manual apply confirmation
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
                auto_apply = sc.auto_apply;
                apply_level = sc.apply_level;
                branch_mode = sc.branch_mode;
                let lang = sc.language.clone();
                config.apply_start_config(sc);
                // Re-initialize state from updated config
                state = OrchestratorState::new(&config);
                state.language = lang;
                let _ = state_tx.send(state.clone());
            }
            Err(_) => {
                info!("开始信号发送端已关闭，退出");
                return Ok(());
            }
        }
    }

    state.session_start = Some(Instant::now());
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
                original_branch = Some(branch);
                let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let new_branch = format!("mdtalk/review-{ts}");
                git_checkout_new_branch(&project_path, &new_branch).await?;
                review_branch = Some(new_branch.clone());
                state.log(&if state.is_en() {
                    format!("Branch mode: created branch {new_branch}")
                } else {
                    format!("分支模式: 已创建分支 {new_branch}")
                });
                initial_dirty_paths = git_status_paths(&project_path).await?;
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

    // === Outer loop: rounds (each round = discussion → consensus → code fix) ===
    for round in 1..=config.review.max_rounds {
        state.current_round = round;
        state.log(&if state.is_en() {
            format!(
                "===== Round {round} started ({} total) =====",
                config.review.max_rounds
            )
        } else {
            format!(
                "===== 第{round}轮审查开始 (共{}轮) =====",
                config.review.max_rounds
            )
        });
        let _ = state_tx.send(state.clone());

        if consume_shutdown_command(&mut cmd_rx, &mut state, &state_tx) {
            expose_branch_info(&mut state, &review_branch, &original_branch);
            finalize_session_state(&mut state, &conversation, &state_tx);
            return Ok(());
        }

        let round_start = Instant::now();
        let mut consensus_reached = false;
        let mut execution_error: Option<anyhow::Error> = None;

        let mut last_a_response = String::new(); // retained for future multi-exchange use
        #[allow(unused_assignments)]
        let mut last_b_response = String::new();

        // === Inner loop: exchanges (A speaks + B speaks + consensus check) ===
        for exchange in 1..=config.review.max_exchanges {
            state.current_exchange = exchange;
            let exchange_kind = classify_exchange(round, exchange);

            if consume_shutdown_command(&mut cmd_rx, &mut state, &state_tx) {
                expose_branch_info(&mut state, &review_branch, &original_branch);
                finalize_session_state(&mut state, &conversation, &state_tx);
                return Ok(());
            }

            // Round header is written once for each outer round.
            if should_append_round_header(exchange) {
                conversation.append_round_header_with_language(round, state.is_en())?;
            }

            // --- Agent A reviews ---
            state.phase = Phase::AgentAReviewing;
            state.log(&if state.is_en() {
                format!("R{round} E{exchange}: Agent A ({}) reviewing", agent_a.name)
            } else {
                format!(
                    "第{round}轮 讨论{exchange}: Agent A ({}) 开始审查",
                    agent_a.name
                )
            });
            let _ = state_tx.send(state.clone());

            let a_prompt = build_agent_a_prompt(exchange_kind, &conv_filename, state.is_en());

            let a_label = if state.is_en() {
                format!(
                    "Round {round} Exchange {exchange}: Agent A ({})",
                    agent_a.name
                )
            } else {
                format!("第{round}轮 讨论{exchange}: Agent A ({})", agent_a.name)
            };
            match run_agent_with_heartbeat(
                &agent_a,
                &a_prompt,
                &project_path,
                &a_label,
                &mut state,
                &state_tx,
            )
            .await
            {
                Ok(output) => {
                    let _ = std::mem::replace(&mut last_a_response, output.content.clone());
                    let label = discussion_role_label(exchange_kind, state.is_en());
                    conversation.append_agent_entry(&agent_a.name, label, &output.content)?;
                    state.log(&if state.is_en() {
                        format!(
                            "R{round} E{exchange}: Agent A done ({:.0}s)",
                            output.duration.as_secs_f64()
                        )
                    } else {
                        format!(
                            "第{round}轮 讨论{exchange}: Agent A 完成 ({:.0}秒)",
                            output.duration.as_secs_f64()
                        )
                    });
                }
                Err(e) => {
                    if state.is_en() {
                        error!("Round {round} Exchange {exchange} Agent A failed: {e}");
                    } else {
                        error!("第{round}轮 讨论{exchange} Agent A 失败: {e}");
                    }
                    state.log(&if state.is_en() {
                        format!("R{round} E{exchange}: Agent A failed: {e}")
                    } else {
                        format!("第{round}轮 讨论{exchange}: Agent A 失败: {e}")
                    });
                    let _ = state_tx.send(state.clone());
                    execution_error = Some(anyhow::anyhow!(if state.is_en() {
                        format!("Round {round} Exchange {exchange}: Agent A execution failed: {e}")
                    } else {
                        format!("第{round}轮 讨论{exchange}: Agent A 执行失败: {e}")
                    }));
                    break;
                }
            }

            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());

            // --- Agent B responds ---
            state.phase = Phase::AgentBResponding;
            state.log(&if state.is_en() {
                format!(
                    "R{round} E{exchange}: Agent B ({}) responding",
                    agent_b.name
                )
            } else {
                format!(
                    "第{round}轮 讨论{exchange}: Agent B ({}) 开始回应",
                    agent_b.name
                )
            });
            let _ = state_tx.send(state.clone());

            let b_prompt = build_agent_b_prompt(&conv_filename, state.is_en());

            let b_label = if state.is_en() {
                format!(
                    "Round {round} Exchange {exchange}: Agent B ({})",
                    agent_b.name
                )
            } else {
                format!("第{round}轮 讨论{exchange}: Agent B ({})", agent_b.name)
            };
            match run_agent_with_heartbeat(
                &agent_b,
                &b_prompt,
                &project_path,
                &b_label,
                &mut state,
                &state_tx,
            )
            .await
            {
                Ok(output) => {
                    last_b_response = output.content.clone();
                    conversation.append_agent_entry(
                        &agent_b.name,
                        if state.is_en() { "Response" } else { "回应" },
                        &output.content,
                    )?;
                    state.log(&if state.is_en() {
                        format!(
                            "R{round} E{exchange}: Agent B done ({:.0}s)",
                            output.duration.as_secs_f64()
                        )
                    } else {
                        format!(
                            "第{round}轮 讨论{exchange}: Agent B 完成 ({:.0}秒)",
                            output.duration.as_secs_f64()
                        )
                    });
                }
                Err(e) => {
                    if state.is_en() {
                        error!("Round {round} Exchange {exchange} Agent B failed: {e}");
                    } else {
                        error!("第{round}轮 讨论{exchange} Agent B 失败: {e}");
                    }
                    state.log(&if state.is_en() {
                        format!("R{round} E{exchange}: Agent B failed: {e}")
                    } else {
                        format!("第{round}轮 讨论{exchange}: Agent B 失败: {e}")
                    });
                    let _ = state_tx.send(state.clone());
                    execution_error = Some(anyhow::anyhow!(if state.is_en() {
                        format!("Round {round} Exchange {exchange}: Agent B execution failed: {e}")
                    } else {
                        format!("第{round}轮 讨论{exchange}: Agent B 执行失败: {e}")
                    }));
                    break;
                }
            }

            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());

            // --- Check consensus ---
            state.phase = Phase::CheckConsensus;
            let _ = state_tx.send(state.clone());

            // Consensus is determined solely by Agent B (the verifier).
            // Agent A proposes issues; it is not expected to say "I agree".
            // Only Agent B, after cross-checking the source, needs to express agreement.
            let result = consensus::ConsensusResult {
                reached: consensus::agent_shows_consensus(
                    &last_b_response,
                    &config.review.consensus_keywords,
                ),
                summary: "Agent B 通过共识关键词确认了审查意见。".to_string(),
            };

            if result.reached {
                state.log(&if state.is_en() {
                    format!("R{round} E{exchange}: consensus reached")
                } else {
                    format!("第{round}轮 讨论{exchange}: 达成共识")
                });
                let summary = if state.is_en() {
                    "Both agents explicitly expressed consensus via the configured keywords."
                        .to_string()
                } else {
                    result.summary.clone()
                };
                conversation.append_consensus_with_language(&summary, state.is_en())?;
                consensus_reached = true;
                break;
            }

            state.log(&if state.is_en() {
                format!("R{round} E{exchange}: no consensus, continuing...")
            } else {
                format!("第{round}轮 讨论{exchange}: 未达成共识，继续讨论...")
            });
            let _ = state_tx.send(state.clone());
        }

        state.round_durations.push(round_start.elapsed());

        if let Some(err) = execution_error {
            expose_branch_info(&mut state, &review_branch, &original_branch);
            finalize_session_state(&mut state, &conversation, &state_tx);
            return Err(err);
        }

        if !consensus_reached {
            // This round failed to reach consensus
            state.phase = Phase::Done;
            state.finished = true;
            state.log(&if state.is_en() {
                format!(
                    "Round {round}: no consensus after {} exchanges, review ended",
                    config.review.max_exchanges
                )
            } else {
                format!(
                    "第{round}轮: {}次讨论后仍未达成共识，审查结束",
                    config.review.max_exchanges
                )
            });
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
        if no_apply {
            state.log(&if state.is_en() {
                format!("Round {round}: skipping apply (--no-apply)")
            } else {
                format!("第{round}轮: 跳过代码修改 (--no-apply)")
            });
            let _ = state_tx.send(state.clone());
        } else {
            if !auto_apply {
                // Manual apply mode: wait for user confirmation
                state.phase = Phase::WaitingForApply;
                state.log(&if state.is_en() {
                    format!("Round {round}: waiting for apply confirmation...")
                } else {
                    format!("第{round}轮: 等待用户确认执行修改...")
                });
                let _ = state_tx.send(state.clone());

                let mut shutdown_requested = false;
                let confirmed = if let Some(ref mut rx) = cmd_rx {
                    loop {
                        match rx.recv().await {
                            Some(OrchestratorCommand::ConfirmApply) => break true,
                            Some(OrchestratorCommand::Shutdown) => {
                                shutdown_requested = true;
                                break false;
                            }
                            Some(_) => {} // ignore other commands
                            None => break true,
                        }
                    }
                } else {
                    true // no channel means auto
                };

                if shutdown_requested {
                    state.log(if state.is_en() {
                        "Shutdown received, ending session"
                    } else {
                        "收到停止信号，提前结束本次会话"
                    });
                    expose_branch_info(&mut state, &review_branch, &original_branch);
                    finalize_session_state(&mut state, &conversation, &state_tx);
                    return Ok(());
                }

                if !confirmed {
                    state.log(&if state.is_en() {
                        format!("Round {round}: user cancelled apply")
                    } else {
                        format!("第{round}轮: 用户取消修改")
                    });
                    let _ = state_tx.send(state.clone());
                    // Skip to next round or finish
                    if round == config.review.max_rounds {
                        state.log(&if state.is_en() {
                            format!("All {} rounds completed", config.review.max_rounds)
                        } else {
                            format!("已完成全部{}轮审查", config.review.max_rounds)
                        });
                    } else {
                        state.log(&if state.is_en() {
                            format!("Round {round} done, next round...")
                        } else {
                            format!("第{round}轮完成，进入下一轮...")
                        });
                        let _ = state_tx.send(state.clone());
                    }
                    continue;
                }

                state.log(&if state.is_en() {
                    format!("Round {round}: user confirmed, applying...")
                } else {
                    format!("第{round}轮: 用户已确认，开始修改...")
                });
            }

            if consume_shutdown_command(&mut cmd_rx, &mut state, &state_tx) {
                expose_branch_info(&mut state, &review_branch, &original_branch);
                finalize_session_state(&mut state, &conversation, &state_tx);
                return Ok(());
            }

            state.phase = Phase::ApplyChanges;
            state.log(&if state.is_en() {
                format!("Round {round}: Agent B applying changes...")
            } else {
                format!("第{round}轮: Agent B 开始根据共识修改代码...")
            });
            let _ = state_tx.send(state.clone());

            let apply_prompt = build_apply_prompt(&conv_filename, apply_level, state.is_en());

            let apply_label = if state.is_en() {
                format!("Round {round} Code Changes: Agent B ({})", agent_b.name)
            } else {
                format!("第{round}轮 代码修改: Agent B ({})", agent_b.name)
            };
            match run_agent_with_heartbeat(
                &agent_b,
                &apply_prompt,
                &project_path,
                &apply_label,
                &mut state,
                &state_tx,
            )
            .await
            {
                Ok(output) => {
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
                        &project_path,
                        round,
                        &output.content,
                        state.is_en(),
                    ) {
                        state.log(&if state.is_en() {
                            format!("Failed to write review_changelog.md: {e}")
                        } else {
                            format!("写入 review_changelog.md 失败: {e}")
                        });
                        expose_branch_info(&mut state, &review_branch, &original_branch);
                        finalize_session_state(&mut state, &conversation, &state_tx);
                        return Err(anyhow::anyhow!(if state.is_en() {
                            format!("Round {round}: failed to write review_changelog.md: {e}")
                        } else {
                            format!("第{round}轮: 写入 review_changelog.md 失败: {e}")
                        }));
                    }
                    state.log(if state.is_en() {
                        "review_changelog.md updated"
                    } else {
                        "review_changelog.md 已更新"
                    });
                    state.log(&if state.is_en() {
                        format!(
                            "Round {round}: Agent B apply done ({:.0}s)",
                            output.duration.as_secs_f64()
                        )
                    } else {
                        format!(
                            "第{round}轮: Agent B 已完成代码修改 ({:.0}秒)",
                            output.duration.as_secs_f64()
                        )
                    });
                }
                Err(e) => {
                    state.log(&if state.is_en() {
                        format!("Round {round}: Agent B apply failed: {e}")
                    } else {
                        format!("第{round}轮: Agent B 修改代码失败: {e}")
                    });
                    expose_branch_info(&mut state, &review_branch, &original_branch);
                    finalize_session_state(&mut state, &conversation, &state_tx);
                    return Err(anyhow::anyhow!(if state.is_en() {
                        format!("Round {round}: Agent B apply failed: {e}")
                    } else {
                        format!("第{round}轮: Agent B 修改代码失败: {e}")
                    }));
                }
            }

            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());
        }

        // Check if this was the last round
        if round == config.review.max_rounds {
            state.log(&if state.is_en() {
                format!("All {} rounds completed", config.review.max_rounds)
            } else {
                format!("已完成全部{}轮审查", config.review.max_rounds)
            });
        } else {
            state.log(&if state.is_en() {
                format!("Round {round} done, next round...")
            } else {
                format!("第{round}轮完成，进入下一轮...")
            });
            let _ = state_tx.send(state.clone());
        }
    }

    // Branch mode: commit changes and optionally wait for merge decision.
    if let (Some(rb), Some(ob)) = (&review_branch, &original_branch) {
        // Always expose branch info so main.rs can print follow-up instructions.
        state.review_branch = Some(rb.clone());
        state.original_branch = Some(ob.clone());

        let commit_outcome = match git_commit_filtered(
            &project_path,
            &format!("mdtalk: review changes on {rb}"),
            &initial_dirty_paths,
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                state.log(&if state.is_en() {
                    format!("Failed to commit changes: {e}")
                } else {
                    format!("提交更改失败: {e}")
                });
                finalize_session_state(&mut state, &conversation, &state_tx);
                return Err(anyhow::anyhow!(if state.is_en() {
                    format!("Branch mode: auto-commit failed: {e}")
                } else {
                    format!("分支模式: 自动提交失败: {e}")
                }));
            }
        };

        match commit_outcome {
            GitCommitOutcome::Committed => {
                state.log(&if state.is_en() {
                    format!("Changes committed on branch {rb}")
                } else {
                    format!("更改已提交到分支 {rb}")
                });

                // Enter WaitingForMerge phase
                state.phase = Phase::WaitingForMerge;
                state.log(if state.is_en() {
                    "Press Enter to merge, or q to keep branch and exit"
                } else {
                    "按 Enter 合并分支，或按 q 保留分支并退出"
                });
                state.update_preview(&conversation);
                let _ = state_tx.send(state.clone());

                // Wait for ConfirmMerge or Shutdown
                let mut do_merge = false;
                if let Some(ref mut rx) = cmd_rx {
                    loop {
                        match rx.recv().await {
                            Some(OrchestratorCommand::ConfirmMerge) => {
                                do_merge = true;
                                break;
                            }
                            Some(OrchestratorCommand::Shutdown) => break,
                            Some(_) => {} // ignore stale commands
                            None => break,
                        }
                    }
                }

                if do_merge {
                    state.log(if state.is_en() {
                        "Merging..."
                    } else {
                        "正在合并..."
                    });
                    let _ = state_tx.send(state.clone());
                    match git_checkout_and_merge(&project_path, ob, rb).await {
                        Ok(()) => {
                            state.log(&if state.is_en() {
                                format!("Merged {rb} into {ob}")
                            } else {
                                format!("已将 {rb} 合并到 {ob}")
                            });
                            // Clear branch info since merge is done.
                            state.review_branch = None;
                            state.original_branch = None;
                        }
                        Err(e) => {
                            state.log(&if state.is_en() {
                                format!("Merge failed: {e}")
                            } else {
                                format!("合并失败: {e}")
                            });
                            // Keep branch info so user can merge manually.
                        }
                    }
                }
            }
            GitCommitOutcome::NothingToCommit => {
                state.log(if state.is_en() {
                    "No file changes to commit on review branch"
                } else {
                    "审查分支无可提交的文件变更"
                });
            }
        }
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::sync::watch;

    use super::{
        ExchangeKind, OrchestratorState, build_agent_a_prompt, build_agent_b_prompt,
        build_apply_prompt, classify_exchange, git_commit_filtered, run,
        should_append_round_header,
    };
    use crate::config::{
        AgentConfig, DashboardConfig, MdtalkConfig, ProjectConfig, ReviewConfig, StartConfig,
    };

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn unique_test_dir(name: &str) -> PathBuf {
        let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mdtalk-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("failed to create test dir");
        dir
    }

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

    fn start_config(agent_a_cmd: String, agent_b_cmd: String, branch_mode: bool) -> StartConfig {
        StartConfig {
            agent_a_command: agent_a_cmd,
            agent_b_command: agent_b_cmd,
            agent_a_timeout_secs: 10,
            agent_b_timeout_secs: 10,
            max_rounds: 1,
            max_exchanges: 1,
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

    fn script_fail_on_second_invocation(dir: &Path, name: &str) -> String {
        #[cfg(windows)]
        {
            write_script(
                dir,
                name,
                "set MARKER_FILE=%~dp0agent_b_called_once.flag\r\nif exist \"%MARKER_FILE%\" (\r\n  echo apply failed 1>&2\r\n  exit /b 1\r\n)\r\necho called>\"%MARKER_FILE%\"\r\necho I agree\r\nexit /b 0",
            )
        }
        #[cfg(unix)]
        {
            write_script(
                dir,
                name,
                "MARKER_FILE=\"$(dirname \"$0\")/agent_b_called_once.flag\"\nif [ -f \"$MARKER_FILE\" ]; then\n  echo \"apply failed\" >&2\n  exit 1\nfi\necho called > \"$MARKER_FILE\"\necho \"I agree\"\nexit 0",
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
        let prompt = build_agent_a_prompt(ExchangeKind::InitialReview, "conversation.md", true);
        assert!(prompt.contains("You are participating in a multi-agent code review workflow"));
        assert!(!prompt.contains("你正在参与"));
    }

    #[test]
    fn english_agent_b_prompt_is_localized() {
        let prompt = build_agent_b_prompt("conversation.md", true);
        assert!(prompt.contains("You are an independent code review expert"));
        assert!(!prompt.contains("你是一位独立的代码审查专家"));
    }

    #[test]
    fn english_apply_prompt_is_localized() {
        let prompt = build_apply_prompt("conversation.md", 1, true);
        assert!(prompt.contains("Consensus has been reached"));
        assert!(!prompt.contains("双方已达成共识"));
    }

    #[tokio::test]
    async fn returns_err_when_agent_a_discussion_fails() {
        let project_dir = unique_test_dir("agent-a-fails");
        let fail_cmd = script_always_fail(&project_dir, "agent_a_fail");
        let ok_cmd = script_always_agree(&project_dir, "agent_b_ok");
        let cfg = test_config(project_dir.clone(), fail_cmd, ok_cmd);
        let (state_tx, _state_rx) = watch::channel(OrchestratorState::new(&cfg));

        let result = run(cfg, state_tx, true, 1, None, None).await;
        assert!(
            result.is_err(),
            "Agent A execution failure should return Err"
        );
        let _ = fs::remove_dir_all(project_dir);
    }

    #[tokio::test]
    async fn returns_err_when_apply_phase_fails() {
        let project_dir = unique_test_dir("apply-fails");
        let a_ok_cmd = script_always_agree(&project_dir, "agent_a_ok");
        let b_cmd = script_fail_on_second_invocation(&project_dir, "agent_b_fail_on_second_call");
        let cfg = test_config(project_dir.clone(), a_ok_cmd, b_cmd);
        let (state_tx, _state_rx) = watch::channel(OrchestratorState::new(&cfg));

        let result = run(cfg, state_tx, false, 1, None, None).await;
        assert!(result.is_err(), "Apply-phase failure should return Err");
        let _ = fs::remove_dir_all(project_dir);
    }

    #[tokio::test]
    async fn branch_mode_errors_when_review_branch_cannot_be_created() {
        let project_dir = unique_test_dir("branch-mode-non-git");
        let a_ok_cmd = script_always_agree(&project_dir, "agent_a_ok");
        let b_ok_cmd = script_always_agree(&project_dir, "agent_b_ok");
        let cfg = test_config(project_dir.clone(), a_ok_cmd.clone(), b_ok_cmd.clone());
        let (state_tx, _state_rx) = watch::channel(OrchestratorState::new(&cfg));
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        start_tx
            .send(start_config(a_ok_cmd, b_ok_cmd, true))
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
        let _ = fs::remove_dir_all(project_dir);
    }

    #[tokio::test]
    async fn branch_mode_commit_excludes_preexisting_dirty_files() {
        let project_dir = unique_test_dir("branch-mode-dirty-filter");
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
            .send(start_config(a_ok_cmd, b_ok_cmd, true))
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

        let _ = fs::remove_dir_all(project_dir);
    }

    #[tokio::test]
    async fn branch_mode_keeps_branch_info_when_no_consensus() {
        let project_dir = unique_test_dir("branch-mode-no-consensus");
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
            .send(start_config(a_cmd, b_cmd, true))
            .expect("failed to send start config");

        let result = run(cfg, state_tx, true, 1, Some(start_rx), None).await;
        assert!(result.is_ok(), "no-consensus path should return Ok");

        let final_state = state_rx.borrow().clone();
        assert!(
            final_state.review_branch.is_some() && final_state.original_branch.is_some(),
            "branch info should still be exposed when branch mode exits early"
        );

        let _ = fs::remove_dir_all(project_dir);
    }

    #[tokio::test]
    async fn git_commit_filtered_returns_error_outside_git_repo() {
        let project_dir = unique_test_dir("git-commit-non-git");
        fs::write(project_dir.join("file.txt"), "content").expect("failed to write test file");

        let result = git_commit_filtered(&project_dir, "test commit", &HashSet::new()).await;
        assert!(
            result.is_err(),
            "git_commit_filtered should fail when git status cannot run"
        );

        let _ = fs::remove_dir_all(project_dir);
    }

    #[tokio::test]
    async fn git_commit_filtered_keeps_unrelated_staged_entries() {
        let project_dir = unique_test_dir("git-commit-keep-staged");
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

        let _ = fs::remove_dir_all(project_dir);
    }

    #[test]
    fn state_logs_are_bounded() {
        let project_dir = unique_test_dir("state-log-cap");
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

        let _ = fs::remove_dir_all(project_dir);
    }
}
