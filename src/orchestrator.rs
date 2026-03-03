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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitCommitOutcome {
    Committed,
    NothingToCommit,
}

/// Stage all changes and commit.
/// Returns whether a commit was created.
async fn git_commit_all(project_path: &Path, message: &str) -> Result<GitCommitOutcome> {
    let add_output = TokioCommand::new("git")
        .args(["add", "-A"])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !add_output.status.success() {
        anyhow::bail!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&add_output.stderr).trim()
        );
    }

    let staged_diff = TokioCommand::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(project_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    match staged_diff.status.code() {
        Some(0) => return Ok(GitCommitOutcome::NothingToCommit),
        Some(1) => {}
        _ => {
            anyhow::bail!(
                "git diff --cached --quiet failed: {}",
                String::from_utf8_lossy(&staged_diff.stderr).trim()
            );
        }
    }

    let commit_output = TokioCommand::new("git")
        .args(["commit", "-m", message])
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await?;
    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&commit_output.stdout).trim().to_string();
        let details = if stderr.is_empty() { stdout } else { stderr };
        anyhow::bail!("git commit -m failed: {details}");
    }

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

/// The state visible to the dashboard.
#[derive(Debug, Clone)]
pub struct OrchestratorState {
    pub phase: Phase,
    pub current_round: u32,
    pub max_rounds: u32,
    pub current_exchange: u32,
    pub max_exchanges: u32,
    pub agent_a_name: String,
    pub agent_b_name: String,
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
                    state.log(if state.is_en() { "Shutdown received, ending session" } else { "收到停止信号，提前结束本次会话" });
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

impl OrchestratorState {
    pub fn new(config: &MdtalkConfig) -> Self {
        Self {
            phase: Phase::Init,
            current_round: 0,
            max_rounds: config.review.max_rounds,
            current_exchange: 0,
            max_exchanges: config.review.max_exchanges,
            agent_a_name: config.agent_a.name.clone(),
            agent_b_name: config.agent_b.name.clone(),
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
    }

    fn update_preview(&mut self, conversation: &Conversation) {
        if let Ok(full) = conversation.read_all() {
            self.conversation_preview = full;
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
    state.log(if state.is_en() { "MDTalk session started" } else { "MDTalk 会话启动" });
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

    // Create conversation file in the project directory
    let conversation = Conversation::new(&project_path, &config.review.output_file, &project_name);
    conversation.create()?;

    let agent_a = AgentRunner::new(&config.agent_a);
    let agent_b = AgentRunner::new(&config.agent_b);
    let conv_filename = config.review.output_file.clone();

    // === Outer loop: rounds (each round = discussion → consensus → code fix) ===
    for round in 1..=config.review.max_rounds {
        state.current_round = round;
        state.log(&if state.is_en() {
            format!("===== Round {round} started ({} total) =====", config.review.max_rounds)
        } else {
            format!("===== 第{round}轮审查开始 (共{}轮) =====", config.review.max_rounds)
        });
        let _ = state_tx.send(state.clone());

        if consume_shutdown_command(&mut cmd_rx, &mut state, &state_tx) {
            return Ok(());
        }

        let round_start = Instant::now();
        let mut consensus_reached = false;
        let mut execution_error: Option<anyhow::Error> = None;

        #[allow(unused_assignments)]
        let mut last_a_response = String::new();
        #[allow(unused_assignments)]
        let mut last_b_response = String::new();

        // === Inner loop: exchanges (A speaks + B speaks + consensus check) ===
        for exchange in 1..=config.review.max_exchanges {
            state.current_exchange = exchange;
            let exchange_kind = classify_exchange(round, exchange);

            if consume_shutdown_command(&mut cmd_rx, &mut state, &state_tx) {
                return Ok(());
            }

            // Round header is written once for each outer round.
            if should_append_round_header(exchange) {
                conversation.append_round_header(round)?;
            }

            // --- Agent A reviews ---
            state.phase = Phase::AgentAReviewing;
            state.log(&if state.is_en() {
                format!("R{round} E{exchange}: Agent A ({}) reviewing", agent_a.name)
            } else {
                format!("第{round}轮 讨论{exchange}: Agent A ({}) 开始审查", agent_a.name)
            });
            let _ = state_tx.send(state.clone());

            let a_prompt = match exchange_kind {
                ExchangeKind::InitialReview => {
                    "你正在参与一个多 agent 代码审查流程。\
                     请仔细阅读当前项目的所有源代码文件（src/ 目录），然后给出详细的审查意见，包括：\n\
                     - 潜在的 bug 和逻辑错误\n\
                     - 代码质量问题\n\
                     - 架构设计问题\n\
                     - 改进建议\n\n\
                     请按优先级排列你的发现。"
                        .to_string()
                }
                ExchangeKind::RoundReReview => {
                    // First exchange of a new round (after code was modified)
                    format!(
                        "你正在参与一个多 agent 代码审查流程。\
                         上一轮审查后代码已被修改。\
                         请先阅读当前目录下的 {conv_filename} 文件了解完整的审查对话历史，\
                         然后重新审查 src/ 目录下的源代码，检查之前发现的问题是否已修复，\
                         以及是否引入了新问题。给出你的审查意见。"
                    )
                }
                ExchangeKind::FollowUp => {
                    format!(
                        "你正在参与一个多 agent 代码审查流程。\
                         请先阅读当前目录下的 {conv_filename} 文件，了解完整的审查对话历史。\n\n\
                         然后根据 Agent B 的最新反馈继续讨论。\
                         表达你是否同意以及你的进一步看法。\
                         如果你已完全同意对方观点，请明确说 \"I agree\" 或 \"达成一致\"。"
                    )
                }
            };

            let a_label = format!("第{round}轮 讨论{exchange}: Agent A ({})", agent_a.name);
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
                    last_a_response = output.content.clone();
                    let label = match exchange_kind {
                        ExchangeKind::InitialReview => "初始审查",
                        ExchangeKind::RoundReReview => "重新审查",
                        ExchangeKind::FollowUp => "后续讨论",
                    };
                    conversation.append_agent_entry(&agent_a.name, label, &output.content)?;
                    state.log(&if state.is_en() {
                        format!("R{round} E{exchange}: Agent A done ({:.0}s)", output.duration.as_secs_f64())
                    } else {
                        format!("第{round}轮 讨论{exchange}: Agent A 完成 ({:.0}秒)", output.duration.as_secs_f64())
                    });
                }
                Err(e) => {
                    error!("第{round}轮 讨论{exchange} Agent A 失败: {e}");
                    state.log(&if state.is_en() {
                        format!("R{round} E{exchange}: Agent A failed: {e}")
                    } else {
                        format!("第{round}轮 讨论{exchange}: Agent A 失败: {e}")
                    });
                    let _ = state_tx.send(state.clone());
                    execution_error = Some(anyhow::anyhow!(
                        "第{round}轮 讨论{exchange}: Agent A 执行失败: {e}"
                    ));
                    break;
                }
            }

            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());

            // --- Agent B responds ---
            state.phase = Phase::AgentBResponding;
            state.log(&if state.is_en() {
                format!("R{round} E{exchange}: Agent B ({}) responding", agent_b.name)
            } else {
                format!("第{round}轮 讨论{exchange}: Agent B ({}) 开始回应", agent_b.name)
            });
            let _ = state_tx.send(state.clone());

            let b_prompt = format!(
                "你是一位独立的代码审查专家。你的任务是对 '{conv_filename}' 中记录的代码审查意见进行逐条验证。\n\n\
                 具体步骤：\n\
                 1. 读取 '{conv_filename}' 文件，找到另一位审查者提出的所有发现\n\
                 2. 对每一条发现，打开对应的源代码文件，核实该问题是否真实存在\n\
                 3. 直接输出你的完整审查回应，格式如下：\n\
                    - 对每条发现标注【同意】或【不同意】，附上你在源代码中看到的证据\n\
                    - 补充任何审查者遗漏的新问题\n\
                    - 在最后给出总结，如果你整体同意，请明确写 \"I agree\" 或 \"同意\"\n\n\
                 重要：你必须直接输出完整的审查文本，不要只报告你读了哪些文件。"
            );

            let b_label = format!("第{round}轮 讨论{exchange}: Agent B ({})", agent_b.name);
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
                    conversation.append_agent_entry(&agent_b.name, "回应", &output.content)?;
                    state.log(&if state.is_en() {
                        format!("R{round} E{exchange}: Agent B done ({:.0}s)", output.duration.as_secs_f64())
                    } else {
                        format!("第{round}轮 讨论{exchange}: Agent B 完成 ({:.0}秒)", output.duration.as_secs_f64())
                    });
                }
                Err(e) => {
                    error!("第{round}轮 讨论{exchange} Agent B 失败: {e}");
                    state.log(&if state.is_en() {
                        format!("R{round} E{exchange}: Agent B failed: {e}")
                    } else {
                        format!("第{round}轮 讨论{exchange}: Agent B 失败: {e}")
                    });
                    let _ = state_tx.send(state.clone());
                    execution_error = Some(anyhow::anyhow!(
                        "第{round}轮 讨论{exchange}: Agent B 执行失败: {e}"
                    ));
                    break;
                }
            }

            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());

            // --- Check consensus ---
            state.phase = Phase::CheckConsensus;
            let _ = state_tx.send(state.clone());

            let result = consensus::check_consensus(
                &last_a_response,
                &last_b_response,
                &config.review.consensus_keywords,
            );

            if result.reached {
                state.log(&if state.is_en() {
                    format!("R{round} E{exchange}: consensus reached")
                } else {
                    format!("第{round}轮 讨论{exchange}: 达成共识")
                });
                conversation.append_consensus(&result.summary)?;
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
            state.phase = Phase::Done;
            state.finished = true;
            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());
            return Err(err);
        }

        if !consensus_reached {
            // This round failed to reach consensus
            state.phase = Phase::Done;
            state.finished = true;
            state.log(&if state.is_en() {
                format!("Round {round}: no consensus after {} exchanges, review ended", config.review.max_exchanges)
            } else {
                format!("第{round}轮: {}次讨论后仍未达成共识，审查结束", config.review.max_exchanges)
            });
            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());
            info!("第{round}轮审查未能达成共识");
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
                    state.phase = Phase::Done;
                    state.finished = true;
                    state.log(if state.is_en() { "Shutdown received, ending session" } else { "收到停止信号，提前结束本次会话" });
                    state.update_preview(&conversation);
                    let _ = state_tx.send(state.clone());
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
                return Ok(());
            }

            // Branch mode: create review branch before the first apply
            if branch_mode && review_branch.is_none() {
                match git_current_branch(&project_path).await {
                    Some(branch) => {
                        original_branch = Some(branch);
                        let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
                        let new_branch = format!("mdtalk/review-{ts}");
                        match git_checkout_new_branch(&project_path, &new_branch).await {
                            Ok(()) => {
                                state.log(&if state.is_en() {
                                    format!("Branch mode: created branch {new_branch}")
                                } else {
                                    format!("分支模式: 已创建分支 {new_branch}")
                                });
                                review_branch = Some(new_branch);
                            }
                            Err(e) => {
                                state.log(&if state.is_en() {
                                    format!("Branch mode: failed to create branch: {e}")
                                } else {
                                    format!("分支模式: 创建分支失败: {e}")
                                });
                                state.update_preview(&conversation);
                                let _ = state_tx.send(state.clone());
                                return Err(anyhow::anyhow!(
                                    "分支模式: 创建审查分支失败，已停止修改: {e}"
                                ));
                            }
                        }
                    }
                    None => {
                        state.log(if state.is_en() {
                            "Branch mode: not a git repo, stopping apply"
                        } else {
                            "分支模式: 非 git 仓库，停止修改"
                        });
                        state.update_preview(&conversation);
                        let _ = state_tx.send(state.clone());
                        return Err(anyhow::anyhow!(
                            "分支模式: 当前目录不是 git 仓库，无法创建隔离分支"
                        ));
                    }
                }
                let _ = state_tx.send(state.clone());
            }

            state.phase = Phase::ApplyChanges;
            state.log(&if state.is_en() {
                format!("Round {round}: Agent B applying changes...")
            } else {
                format!("第{round}轮: Agent B 开始根据共识修改代码...")
            });
            let _ = state_tx.send(state.clone());

            let apply_instruction = match apply_level {
                2 => "选择高优先级和中优先级问题，阅读相关的源代码文件并直接修改代码来修复这些问题。低优先级问题暂不处理。",
                3 => "修复所有已达成共识的问题，阅读相关的源代码文件并直接修改代码。",
                _ => "选择所有高优先级问题，阅读相关的源代码文件并直接修改代码来修复。中低优先级问题暂不处理。",
            };
            let apply_prompt = format!(
                "双方已达成共识。请先阅读当前目录下的 {conv_filename} 文件了解完整审查对话，\
                 然后根据讨论中达成一致的改进意见，{apply_instruction}"
            );

            let apply_label = format!("第{round}轮 代码修改: Agent B ({})", agent_b.name);
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
                    conversation.append_agent_entry(&agent_b.name, "代码修改", &output.content)?;
                    if let Err(e) =
                        crate::conversation::append_changelog(&project_path, round, &output.content)
                    {
                        state.log(&if state.is_en() {
                        format!("Failed to write review_changelog.md: {e}")
                    } else {
                        format!("写入 review_changelog.md 失败: {e}")
                    });
                        state.update_preview(&conversation);
                        let _ = state_tx.send(state.clone());
                        return Err(anyhow::anyhow!(
                            "第{round}轮: 写入 review_changelog.md 失败: {e}"
                        ));
                    }
                    state.log(if state.is_en() { "review_changelog.md updated" } else { "review_changelog.md 已更新" });
                    state.log(&if state.is_en() {
                        format!("Round {round}: Agent B apply done ({:.0}s)", output.duration.as_secs_f64())
                    } else {
                        format!("第{round}轮: Agent B 已完成代码修改 ({:.0}秒)", output.duration.as_secs_f64())
                    });
                }
                Err(e) => {
                    state.log(&if state.is_en() {
                        format!("Round {round}: Agent B apply failed: {e}")
                    } else {
                        format!("第{round}轮: Agent B 修改代码失败: {e}")
                    });
                    state.update_preview(&conversation);
                    let _ = state_tx.send(state.clone());
                    return Err(anyhow::anyhow!("第{round}轮: Agent B 修改代码失败: {e}"));
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

    // Branch mode: commit changes and wait for merge decision
    if let (Some(rb), Some(ob)) = (&review_branch, &original_branch) {
        // Auto-commit all changes on the review branch
        match git_commit_all(&project_path, &format!("mdtalk: review changes on {rb}")).await {
            Ok(GitCommitOutcome::Committed) => {
                state.log(&if state.is_en() {
                    format!("Changes committed on branch {rb}")
                } else {
                    format!("更改已提交到分支 {rb}")
                });
            }
            Ok(GitCommitOutcome::NothingToCommit) => {
                state.log(if state.is_en() {
                    "No file changes to commit on review branch"
                } else {
                    "审查分支无可提交的文件变更"
                });
            }
            Err(e) => {
                state.log(&if state.is_en() {
                    format!("Failed to commit changes: {e}")
                } else {
                    format!("提交更改失败: {e}")
                });
            }
        }

        // Enter WaitingForMerge phase
        state.phase = Phase::WaitingForMerge;
        state.review_branch = Some(rb.clone());
        state.original_branch = Some(ob.clone());
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
            state.log(if state.is_en() { "Merging..." } else { "正在合并..." });
            let _ = state_tx.send(state.clone());
            match git_checkout_and_merge(&project_path, ob, rb).await {
                Ok(()) => {
                    state.log(&if state.is_en() {
                        format!("Merged {rb} into {ob}")
                    } else {
                        format!("已将 {rb} 合并到 {ob}")
                    });
                    // Clear branch info since merge is done
                    state.review_branch = None;
                    state.original_branch = None;
                }
                Err(e) => {
                    state.log(&if state.is_en() {
                        format!("Merge failed: {e}")
                    } else {
                        format!("合并失败: {e}")
                    });
                    // Keep branch info so user can merge manually
                }
            }
        }
        // If not merging, review_branch/original_branch stay set for main.rs to print instructions
    }

    state.phase = Phase::Done;
    state.finished = true;
    state.update_preview(&conversation);
    let _ = state_tx.send(state.clone());
    info!("审查会话完成 (共{}轮)", config.review.max_rounds);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::sync::watch;

    use super::{
        ExchangeKind, OrchestratorState, classify_exchange, git_commit_all, run,
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
            return write_script(dir, name, "echo failed 1>&2\r\nexit /b 1");
        }
        #[cfg(unix)]
        {
            return write_script(dir, name, "echo failed >&2\nexit 1");
        }
    }

    fn script_always_agree(dir: &Path, name: &str) -> String {
        #[cfg(windows)]
        {
            return write_script(dir, name, "echo I agree\r\nexit /b 0");
        }
        #[cfg(unix)]
        {
            return write_script(dir, name, "echo \"I agree\"\nexit 0");
        }
    }

    fn script_fail_on_second_invocation(dir: &Path, name: &str) -> String {
        #[cfg(windows)]
        {
            return write_script(
                dir,
                name,
                "set MARKER_FILE=%~dp0agent_b_called_once.flag\r\nif exist \"%MARKER_FILE%\" (\r\n  echo apply failed 1>&2\r\n  exit /b 1\r\n)\r\necho called>\"%MARKER_FILE%\"\r\necho I agree\r\nexit /b 0",
            );
        }
        #[cfg(unix)]
        {
            return write_script(
                dir,
                name,
                "MARKER_FILE=\"$(dirname \"$0\")/agent_b_called_once.flag\"\nif [ -f \"$MARKER_FILE\" ]; then\n  echo \"apply failed\" >&2\n  exit 1\nfi\necho called > \"$MARKER_FILE\"\necho \"I agree\"\nexit 0",
            );
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
        let _ = fs::remove_dir_all(project_dir);
    }

    #[tokio::test]
    async fn git_commit_all_returns_error_outside_git_repo() {
        let project_dir = unique_test_dir("git-commit-non-git");
        fs::write(project_dir.join("file.txt"), "content").expect("failed to write test file");

        let result = git_commit_all(&project_dir, "test commit").await;
        assert!(
            result.is_err(),
            "git_commit_all should fail when git add/commit fails"
        );

        let _ = fs::remove_dir_all(project_dir);
    }
}
