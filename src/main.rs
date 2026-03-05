mod agent;
mod config;
mod consensus;
mod conversation;
mod dashboard;
mod orchestrator;
#[cfg(test)]
mod test_utils;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "mdtalk",
    about = "Multi-agent code review via Markdown conversation",
    disable_version_flag = true
)]
struct Cli {
    /// Print version/build information and executable path
    #[arg(short = 'V', long = "version", action = clap::ArgAction::SetTrue)]
    version: bool,

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

fn print_version_info() {
    let git_commit = option_env!("MDTALK_GIT_COMMIT").unwrap_or("unknown");
    let build_unix = option_env!("MDTALK_BUILD_UNIX").unwrap_or("unknown");
    let exe_path = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    println!("mdtalk {}", env!("CARGO_PKG_VERSION"));
    println!("git_commit: {git_commit}");
    println!("build_unix: {build_unix}");
    println!("executable: {exe_path}");
}

fn apply_restart_defaults(
    cfg: &mut config::MdtalkConfig,
    no_apply: &mut bool,
    apply_level: &mut u32,
    final_state: &orchestrator::OrchestratorState,
) {
    cfg.agent_a.name = final_state.agent_a_name.clone();
    cfg.agent_a.command = final_state.agent_a_name.clone();
    cfg.agent_a.timeout_secs = final_state.agent_a_timeout_secs;
    cfg.agent_b.name = final_state.agent_b_name.clone();
    cfg.agent_b.command = final_state.agent_b_name.clone();
    cfg.agent_b.timeout_secs = final_state.agent_b_timeout_secs;
    cfg.review.max_rounds = final_state.max_rounds;
    cfg.review.max_exchanges = final_state.max_exchanges;
    *no_apply = final_state.no_apply;
    *apply_level = final_state.apply_level;
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.version {
        print_version_info();
        return Ok(());
    }

    if cli.demo {
        return dashboard::render_demo();
    }

    // Load config base:
    // - if --config is provided: load it explicitly
    // - else require --project and auto-load <project>/mdtalk.toml when present
    let mut cfg = if let Some(config_path) = &cli.config {
        config::MdtalkConfig::load(config_path)?
    } else if let Some(project_path) = &cli.project {
        config::MdtalkConfig::from_project_with_optional_config(project_path.clone())?
    } else {
        anyhow::bail!("未指定项目。请使用 --project <路径> 或 --config <路径>。");
    };

    // CLI overrides always win over defaults/file.
    cfg.apply_cli_overrides(
        cli.project.clone(),
        cli.agent_a.clone(),
        cli.agent_b.clone(),
        cli.max_rounds,
        cli.max_exchanges,
    )?;

    if cli.no_dashboard {
        // No-dashboard mode: set up tracing to stdout and just run the orchestrator
        tracing_subscriber::fmt()
            .with_env_filter("mdtalk=info")
            .init();

        let mut initial_state = orchestrator::OrchestratorState::new(&cfg);
        initial_state.no_apply = cli.no_apply;
        initial_state.apply_level = cli.apply_level;
        let (state_tx, _state_rx) = watch::channel(initial_state);
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
                tracing_subscriber::fmt()
                    .with_env_filter("mdtalk=info")
                    .with_writer(std::io::stderr)
                    .with_ansi(false)
                    .init();
            }
        }

        let mut no_apply = cli.no_apply;
        let mut apply_level = cli.apply_level;
        let refresh_rate_ms = cfg.dashboard.refresh_rate_ms;

        loop {
            let cfg_clone = cfg.clone();
            let mut initial_state = orchestrator::OrchestratorState::new(&cfg_clone);
            initial_state.no_apply = no_apply;
            initial_state.apply_level = apply_level;
            let (state_tx, state_rx) = watch::channel(initial_state);
            let (start_tx, start_rx) = oneshot::channel::<config::StartConfig>();
            let (cmd_tx, cmd_rx) = mpsc::channel::<orchestrator::OrchestratorCommand>(1);
            let cmd_tx_shutdown = cmd_tx.clone();

            let run_no_apply = no_apply;
            let run_apply_level = apply_level;
            let mut orchestrator_handle = tokio::spawn(async move {
                orchestrator::run(
                    cfg_clone,
                    state_tx,
                    run_no_apply,
                    run_apply_level,
                    Some(start_rx),
                    Some(cmd_rx),
                )
                .await
            });

            let state_rx_main = state_rx.clone();
            let dashboard_handle = tokio::task::spawn_blocking(move || {
                dashboard::run(state_rx, start_tx, cmd_tx, refresh_rate_ms)
            });

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
            let final_state = state_rx_main.borrow().clone();
            if let (Some(rb), Some(ob)) = (&final_state.review_branch, &final_state.original_branch)
            {
                eprintln!();
                eprintln!("─── Branch Mode ───");
                eprintln!("Changes on branch: {rb}");
                eprintln!("To review:  git diff {ob}..{rb}");
                eprintln!("To merge:   git checkout {ob} && git merge {rb}");
                eprintln!();
            }

            // Keep the most recent runtime choices as defaults for restart.
            apply_restart_defaults(&mut cfg, &mut no_apply, &mut apply_level, &final_state);

            match exit {
                dashboard::DashboardExit::Restart => continue,
                dashboard::DashboardExit::Quit => break,
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::apply_restart_defaults;
    use crate::{config, orchestrator};

    #[test]
    fn restart_defaults_preserve_apply_level_and_no_apply() {
        let mut cfg = config::MdtalkConfig::from_cli(
            PathBuf::from("."),
            Some("claude".to_string()),
            Some("codex".to_string()),
            Some(1),
            Some(1),
        );
        let mut no_apply = false;
        let mut apply_level = 1;
        let mut final_state = orchestrator::OrchestratorState::new(&cfg);
        final_state.agent_a_name = "agent-a-override".to_string();
        final_state.agent_a_timeout_secs = 120;
        final_state.agent_b_name = "agent-b-override".to_string();
        final_state.agent_b_timeout_secs = 180;
        final_state.max_rounds = 4;
        final_state.max_exchanges = 6;
        final_state.no_apply = true;
        final_state.apply_level = 3;

        apply_restart_defaults(&mut cfg, &mut no_apply, &mut apply_level, &final_state);

        assert_eq!(cfg.agent_a.command, "agent-a-override");
        assert_eq!(cfg.agent_b.command, "agent-b-override");
        assert_eq!(cfg.review.max_rounds, 4);
        assert_eq!(cfg.review.max_exchanges, 6);
        assert!(no_apply, "restart should retain no-apply choice");
        assert_eq!(
            apply_level, 3,
            "restart should retain start-screen apply level choice"
        );
    }
}
