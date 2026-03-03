mod agent;
mod config;
mod consensus;
mod conversation;
mod dashboard;
mod orchestrator;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "mdtalk",
    about = "Multi-agent code review via Markdown conversation"
)]
struct Cli {
    /// Path to the project to review
    #[arg(short, long)]
    project: Option<PathBuf>,

    /// Path to mdtalk.toml config file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Command for Agent A (default: claude)
    #[arg(long, value_name = "CMD")]
    agent_a: Option<String>,

    /// Command for Agent B (default: codex)
    #[arg(long, value_name = "CMD")]
    agent_b: Option<String>,

    /// Maximum number of review rounds (each round = consensus + code fix)
    #[arg(short, long, value_parser = clap::value_parser!(u32).range(1..))]
    max_rounds: Option<u32>,

    /// Maximum exchanges per round (A+B back-and-forth before giving up)
    #[arg(short = 'e', long, value_parser = clap::value_parser!(u32).range(1..))]
    max_exchanges: Option<u32>,

    /// Run without TUI dashboard (log to stdout)
    #[arg(long)]
    no_dashboard: bool,

    /// Skip the "apply changes" phase after consensus
    #[arg(long)]
    no_apply: bool,

    /// Apply severity level: 1=高 only, 2=高+中, 3=all (default: 1)
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=3), default_value = "1")]
    apply_level: u32,

    /// Render one dashboard frame with mock data and exit (for preview)
    #[arg(long)]
    demo: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.demo {
        return dashboard::render_demo();
    }

    // Load config: from file if provided, otherwise from CLI args
    let cfg = if let Some(config_path) = &cli.config {
        config::MdtalkConfig::load(config_path)?
    } else if let Some(project_path) = &cli.project {
        config::MdtalkConfig::from_cli(
            project_path.clone(),
            cli.agent_a.clone(),
            cli.agent_b.clone(),
            cli.max_rounds,
            cli.max_exchanges,
        )
    } else {
        // Try loading from default mdtalk.toml in current directory
        let default_path = PathBuf::from("mdtalk.toml");
        if default_path.exists() {
            config::MdtalkConfig::load(&default_path)?
        } else {
            anyhow::bail!(
                "未指定项目。请使用 --project <路径> 或 --config <路径>，\
                 或在当前目录创建 mdtalk.toml 配置文件。"
            );
        }
    };

    if cli.no_dashboard {
        // No-dashboard mode: set up tracing to stdout and just run the orchestrator
        tracing_subscriber::fmt()
            .with_env_filter("mdtalk=info")
            .init();

        let (state_tx, _state_rx) = watch::channel(orchestrator::OrchestratorState::new(&cfg));
        info!("MDTalk 审查启动 (无仪表盘模式)");
        orchestrator::run(cfg, state_tx, cli.no_apply, cli.apply_level, None, None).await?;
    } else {
        // Dashboard mode: tracing goes to a log file
        match std::fs::File::create("mdtalk.log") {
            Ok(file) => {
                // Use LineWriter to flush after every line, so logs survive process abort
                let writer = std::sync::Mutex::new(std::io::LineWriter::new(file));
                tracing_subscriber::fmt()
                    .with_env_filter("mdtalk=info")
                    .with_writer(writer)
                    .with_ansi(false)
                    .init();
            }
            Err(e) => {
                eprintln!("警告: 无法创建日志文件 mdtalk.log: {e}");
                // Fall back to no logging in dashboard mode
            }
        }

        let no_apply = cli.no_apply;

        loop {
            let cfg_clone = cfg.clone();
            let (state_tx, state_rx) =
                watch::channel(orchestrator::OrchestratorState::new(&cfg_clone));
            let (start_tx, start_rx) = oneshot::channel::<config::StartConfig>();
            let (cmd_tx, cmd_rx) = mpsc::channel::<orchestrator::OrchestratorCommand>(1);
            let cmd_tx_shutdown = cmd_tx.clone();

            let apply_level = cli.apply_level;
            let mut orchestrator_handle = tokio::spawn(async move {
                orchestrator::run(cfg_clone, state_tx, no_apply, apply_level, Some(start_rx), Some(cmd_rx)).await
            });

            let state_rx_main = state_rx.clone();
            let dashboard_handle =
                tokio::task::spawn_blocking(move || dashboard::run(state_rx, start_tx, cmd_tx));

            // Wait for dashboard to finish (user presses q or orchestrator sets finished).
            // Then request graceful orchestrator shutdown.
            let orch_abort = orchestrator_handle.abort_handle();

            let dash_result = dashboard_handle.await;
            let exit = match dash_result {
                Ok(Ok(exit)) => exit,
                Ok(Err(e)) => {
                    eprintln!("Dashboard error: {e}");
                    dashboard::DashboardExit::Quit
                }
                Err(e) => {
                    eprintln!("Dashboard panic: {e}");
                    dashboard::DashboardExit::Quit
                }
            };

            // Dashboard exited — request graceful stop first, then force abort on timeout.
            if !orchestrator_handle.is_finished() {
                let _ = cmd_tx_shutdown.try_send(orchestrator::OrchestratorCommand::Shutdown);
            }

            match tokio::time::timeout(Duration::from_secs(3), &mut orchestrator_handle).await {
                Ok(join_result) => match join_result {
                    Ok(Err(e)) => eprintln!("Orchestrator error: {e}"),
                    Err(e) => eprintln!("Orchestrator panic: {e}"),
                    _ => {}
                },
                Err(_) => {
                    orch_abort.abort();
                    match orchestrator_handle.await {
                        Ok(Err(e)) => eprintln!("Orchestrator error: {e}"),
                        Err(e) if e.is_cancelled() => {} // expected if we aborted
                        Err(e) => eprintln!("Orchestrator panic: {e}"),
                        _ => {}
                    }
                }
            }

            // Print merge instructions if branch was kept (not merged)
            {
                let final_state = state_rx_main.borrow();
                if let (Some(rb), Some(ob)) = (&final_state.review_branch, &final_state.original_branch) {
                    eprintln!();
                    eprintln!("─── Branch Mode ───");
                    eprintln!("Changes on branch: {rb}");
                    eprintln!("To review:  git diff {ob}..{rb}");
                    eprintln!("To merge:   git checkout {ob} && git merge {rb}");
                    eprintln!();
                }
            }

            match exit {
                dashboard::DashboardExit::Restart => continue,
                dashboard::DashboardExit::Quit => break,
            }
        }
    }

    Ok(())
}
