pub mod app;
pub mod events;
pub mod ui;

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::{CrosstermBackend, TestBackend};
use tokio::sync::{oneshot, watch};

use self::app::DashboardApp;
use crate::orchestrator::{OrchestratorState, Phase};

/// Restore terminal to normal mode. Called on both normal exit and error.
fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    );
    let _ = terminal.show_cursor();
}

/// Run the TUI dashboard. This blocks until the user quits or the orchestrator finishes.
/// NOTE: This is intentionally a blocking (non-async) function because crossterm's
/// event::poll() blocks the OS thread. It must run on spawn_blocking, not tokio::spawn.
pub fn run(
    mut state_rx: watch::Receiver<OrchestratorState>,
    start_tx: oneshot::Sender<()>,
) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_dashboard_loop(&mut terminal, &mut state_rx, start_tx);

    // Always restore terminal, even on error
    restore_terminal(&mut terminal);

    result
}

fn run_dashboard_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state_rx: &mut watch::Receiver<OrchestratorState>,
    start_tx: oneshot::Sender<()>,
) -> Result<()> {
    // Drain any buffered key events from launching the command
    events::drain_buffered_events();

    let initial_state = state_rx.borrow().clone();
    let mut app = DashboardApp::new(initial_state, start_tx);

    let tick_rate = Duration::from_millis(100);

    loop {
        // Draw
        terminal.draw(|f| ui::draw(f, &app))?;

        // Handle input events
        events::handle_events(&mut app, tick_rate)?;

        // Check for state updates from orchestrator.
        // has_changed() returns Err when the sender is dropped (orchestrator finished).
        // In that case we must still read the final state to avoid missing the Done update.
        let state_changed = match state_rx.has_changed() {
            Ok(changed) => changed,
            Err(_) => {
                // Sender dropped — read the final value if we haven't seen "finished" yet
                !app.state.finished
            }
        };

        if state_changed {
            let new_state = state_rx.borrow_and_update().clone();
            let finished = new_state.finished;
            app.update_state(new_state);

            if finished {
                terminal.draw(|f| ui::draw(f, &app))?;
                loop {
                    events::handle_events(&mut app, Duration::from_millis(200))?;
                    if app.should_quit {
                        break;
                    }
                    terminal.draw(|f| ui::draw(f, &app))?;
                }
                break;
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Render one frame with mock data to stdout (for `--demo` preview).
pub fn render_demo() -> Result<()> {
    let mock_state = OrchestratorState {
        phase: Phase::AgentBResponding,
        current_round: 1,
        max_rounds: 2,
        current_exchange: 2,
        max_exchanges: 5,
        agent_a_name: "claude".to_string(),
        agent_b_name: "codex".to_string(),
        round_durations: vec![Duration::from_secs(150)],
        session_start: Some(Instant::now() - Duration::from_secs(222)),
        logs: vec![
            "[13:30:00] MDTalk 会话启动".to_string(),
            "[13:30:01] 第1轮: Agent A (claude) 开始审查".to_string(),
            "[13:32:31] 第1轮: Agent A 完成 (150秒)".to_string(),
            "[13:32:31] 第1轮: Agent B (codex) 开始回应".to_string(),
            "[13:36:42] 第1轮: Agent B 完成 (251秒)".to_string(),
            "[13:36:42] 第1轮: 未达成共识，继续...".to_string(),
            "[13:36:43] 第2轮: Agent A (claude) 开始审查".to_string(),
            "[13:38:10] 第2轮: Agent A 完成 (87秒)".to_string(),
            "[13:38:10] 第2轮: Agent B (codex) 开始回应".to_string(),
        ],
        conversation_preview: "\
# Code Review: my-project
## Review Session - 2026-03-03 13:30:00

### 第1轮

#### claude - 初始审查 [13:32:31]

## 代码审查报告

### 1. [高] agent.rs:88 潜在死锁
wait() 后读取 stdout/stderr 可能因管道缓冲区满而死锁。

### 2. [高] 超时未终止子进程
超时丢弃 future 但未 kill 子进程。

### 3. [中] 共识检测过于宽松
\"agree\" 等关键词在 \"I don't agree\" 中也会匹配。

---

#### codex - 回应 [13:36:42]

**审查意见验证**
1. 同意 - 确认 agent.rs 中存在死锁风险
2. 同意 - 超时时需要 child.kill()
3. 同意 - 需要否定检测

**补充问题**
1. consensus.rs 中 UTF-8 字节切片可能 panic

---

### 第2轮

#### claude - 后续讨论 [13:38:10]

我同意 codex 的所有发现。UTF-8 问题
确实值得关注。另外补充一点...

---"
        .to_string(),
        finished: false,
    };

    let (start_tx, _start_rx) = oneshot::channel();
    let mut app = DashboardApp::new(mock_state, start_tx);
    // Demo shows the running state, not the start screen
    app.waiting_for_start = false;

    // Render to a test backend buffer (80x24 terminal)
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|f| ui::draw(f, &app))?;

    // Print the buffer content
    let buffer = terminal.backend().buffer().clone();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            line.push_str(cell.symbol());
        }
        println!("{}", line.trim_end());
    }

    Ok(())
}
