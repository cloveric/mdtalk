use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::watch;
use tracing::{error, info};

use crate::agent::{AgentOutput, AgentRunner};
use crate::config::MdtalkConfig;
use crate::consensus;
use crate::conversation::Conversation;

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
                state.log(&format!("{label} 运行中... (已{elapsed}秒)"));
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Init,
    AgentAReviewing,
    AgentBResponding,
    CheckConsensus,
    ApplyChanges,
    Done,
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Phase::Init => write!(f, "初始化"),
            Phase::AgentAReviewing => write!(f, "Agent A 审查中"),
            Phase::AgentBResponding => write!(f, "Agent B 回应中"),
            Phase::CheckConsensus => write!(f, "检测共识"),
            Phase::ApplyChanges => write!(f, "修改代码中"),
            Phase::Done => write!(f, "已完成"),
        }
    }
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
        }
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
    start_rx: Option<tokio::sync::oneshot::Receiver<crate::config::StartConfig>>,
) -> Result<()> {
    let mut state = OrchestratorState::new(&config);
    info!("编排器已启动");

    // Wait for dashboard confirmation if a start signal receiver is provided
    if let Some(rx) = start_rx {
        info!("等待用户确认开始...");
        match rx.await {
            Ok(sc) => {
                info!("收到开始信号");
                config.apply_start_config(sc);
                // Re-initialize state from updated config
                state = OrchestratorState::new(&config);
                let _ = state_tx.send(state.clone());
            }
            Err(_) => {
                info!("开始信号发送端已关闭，退出");
                return Ok(());
            }
        }
    }

    state.session_start = Some(Instant::now());
    state.log("MDTalk 会话启动");
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
        state.log(&format!(
            "===== 第{round}轮审查开始 (共{}轮) =====",
            config.review.max_rounds
        ));
        let _ = state_tx.send(state.clone());

        let round_start = Instant::now();
        let mut consensus_reached = false;

        #[allow(unused_assignments)]
        let mut last_a_response = String::new();
        #[allow(unused_assignments)]
        let mut last_b_response = String::new();

        // === Inner loop: exchanges (A speaks + B speaks + consensus check) ===
        for exchange in 1..=config.review.max_exchanges {
            state.current_exchange = exchange;
            let exchange_kind = classify_exchange(round, exchange);

            // Round header is written once for each outer round.
            if should_append_round_header(exchange) {
                conversation.append_round_header(round)?;
            }

            // --- Agent A reviews ---
            state.phase = Phase::AgentAReviewing;
            state.log(&format!(
                "第{round}轮 讨论{exchange}: Agent A ({}) 开始审查",
                agent_a.name
            ));
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
                    state.log(&format!(
                        "第{round}轮 讨论{exchange}: Agent A 完成 ({:.0}秒)",
                        output.duration.as_secs_f64()
                    ));
                }
                Err(e) => {
                    error!("第{round}轮 讨论{exchange} Agent A 失败: {e}");
                    state.log(&format!("第{round}轮 讨论{exchange}: Agent A 失败: {e}"));
                    let _ = state_tx.send(state.clone());
                    break;
                }
            }

            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());

            // --- Agent B responds ---
            state.phase = Phase::AgentBResponding;
            state.log(&format!(
                "第{round}轮 讨论{exchange}: Agent B ({}) 开始回应",
                agent_b.name
            ));
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
                    state.log(&format!(
                        "第{round}轮 讨论{exchange}: Agent B 完成 ({:.0}秒)",
                        output.duration.as_secs_f64()
                    ));
                }
                Err(e) => {
                    error!("第{round}轮 讨论{exchange} Agent B 失败: {e}");
                    state.log(&format!("第{round}轮 讨论{exchange}: Agent B 失败: {e}"));
                    let _ = state_tx.send(state.clone());
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
                state.log(&format!("第{round}轮 讨论{exchange}: 达成共识"));
                conversation.append_consensus(&result.summary)?;
                consensus_reached = true;
                break;
            }

            state.log(&format!(
                "第{round}轮 讨论{exchange}: 未达成共识，继续讨论..."
            ));
            let _ = state_tx.send(state.clone());
        }

        state.round_durations.push(round_start.elapsed());

        if !consensus_reached {
            // This round failed to reach consensus
            state.phase = Phase::Done;
            state.finished = true;
            state.log(&format!(
                "第{round}轮: {}次讨论后仍未达成共识，审查结束",
                config.review.max_exchanges
            ));
            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());
            info!("第{round}轮审查未能达成共识");
            return Ok(());
        }

        // === Consensus reached — apply changes ===
        if no_apply {
            state.log(&format!("第{round}轮: 跳过代码修改 (--no-apply)"));
            let _ = state_tx.send(state.clone());
        } else {
            state.phase = Phase::ApplyChanges;
            state.log(&format!("第{round}轮: Agent B 开始根据共识修改代码..."));
            let _ = state_tx.send(state.clone());

            let apply_prompt = format!(
                "双方已达成共识。请先阅读当前目录下的 {conv_filename} 文件了解完整审查对话，\
                 然后根据讨论中达成一致的改进意见，只选择最重要的 3 个高优先级问题，\
                 阅读相关的源代码文件并直接修改代码来修复这 3 个问题。不要尝试修复所有问题。"
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
                    state.log(&format!(
                        "第{round}轮: Agent B 已完成代码修改 ({:.0}秒)",
                        output.duration.as_secs_f64()
                    ));
                }
                Err(e) => {
                    state.log(&format!("第{round}轮: Agent B 修改代码失败: {e}"));
                }
            }

            state.update_preview(&conversation);
            let _ = state_tx.send(state.clone());
        }

        // Check if this was the last round
        if round == config.review.max_rounds {
            state.log(&format!("已完成全部{}轮审查", config.review.max_rounds));
        } else {
            state.log(&format!("第{round}轮完成，进入下一轮..."));
            let _ = state_tx.send(state.clone());
        }
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
    use super::{ExchangeKind, classify_exchange, should_append_round_header};

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
}
