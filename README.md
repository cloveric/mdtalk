<div align="center">

# MDTalk

### AI-Powered Multi-Agent Code Review / AI 驱动的多智能体代码审查

**One AI reviewing its own code finds nothing. Two AIs debating it find everything.**

**一个 AI 自检查不出问题，两个 AI 互相争论就全找到了。**

[![Rust](https://img.shields.io/badge/Rust-1.75+-orange?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)](https://github.com/cloveric/mdtalk)
[![License](https://img.shields.io/badge/License-MIT-green)](LICENSE)
[![Status](https://img.shields.io/badge/Status-Active-brightgreen)](https://github.com/cloveric/mdtalk)

[English](#overview) | [中文](#概述)

</div>

---

## Overview

You've just finished a feature. You ask your AI to review the code. It says it looks great. But the real bug is still there — **the same AI that wrote it can't properly review it**.

MDTalk solves this by orchestrating two independent AI agents into a structured debate. Agent A reviews your code, Agent B challenges every finding against the actual source, and they go back and forth until they reach consensus. Then Agent B applies the agreed fixes directly to your codebase.

## 概述

你刚写完一个功能，让 AI 检查代码，它说挺好的。但 bug 还在 — **写代码的 AI 检查不出自己的问题**。

MDTalk 的解决方案：让两个独立的 AI agent 进行结构化辩论。Agent A 审查代码，Agent B 逐条验证每个发现，双方反复讨论直到达成共识。然后 Agent B 将共识修复直接应用到代码中。

---

## Live TUI Dashboard / 实时仪表盘

```
┌ MDTalk 仪表盘 ──────────────────────────────────────────────────────────────┐
│ 状态: Agent B (codex) 回应中  │  轮次: 1/2  │  讨论: 2/5                    │
│ 已用时: 00:03:42  │  按 q 退出, ↑↓ 滚动                                    │
└─────────────────────────────────────────────────────────────────────────────┘
┌ 对话预览 ──────────────────────────────────┐┌ Agent 状态 ────────────────┐
│ # Code Review: my-project                  ││ Agent A: claude  ○ 等待中  │
│ ## Review Session - 2026-03-03 13:30:00    ││ Agent B: codex   ● 回应中  │
│                                            ││                           │
│ ### 第1轮                                  ││ 轮次耗时:                  │
│                                            ││   第1轮: 2分30秒           │
│ #### claude - 初始审查 [13:32:31]          ││                           │
│                                            ││                           │
│ ## 代码审查报告                            ││                           │
│                                            ││                           │
│ ### 1. [高] agent.rs:88 潜在死锁           ││                           │
│ wait() 后读取 stdout/stderr                ││                           │
│ 可能因管道缓冲区满而死锁。                ││                           │
└────────────────────────────────────────────┘└───────────────────────────┘
┌ 日志 ───────────────────────────────────────────────────────────────────────┐
│ [13:30:00] MDTalk 会话启动                                                  │
│ [13:30:01] 第1轮: Agent A (claude) 开始审查                                 │
│ [13:32:31] 第1轮: Agent A 完成 (150秒)                                      │
│ [13:32:31] 第1轮: Agent B (codex) 开始回应                                  │
└─────────────────────────────────────────────────────────────────────────────┘
```

> Preview with `mdtalk --demo` / 使用 `mdtalk --demo` 预览

---

## How It Works / 工作原理

```
┌────────────────── Round Loop (max_rounds) ──────────────────┐
│                                                              │
│  ┌─────────── Exchange Loop (max_exchanges) ─────────────┐  │
│  │                                                        │  │
│  │  Agent A ── reviews src/ ──────────────────────►      │  │
│  │                                      writes to ▼      │  │
│  │  Agent B ◄── reads conversation.md ── conversation    │  │
│  │          ── verifies each finding ──────────────►     │  │
│  │                                      writes to ▼      │  │
│  │              Consensus Check ◄─────── conversation    │  │
│  │                /         \                             │  │
│  │          Agreed ✓     Disagree → next exchange         │  │
│  └────────────────────────────────────────────────────────┘  │
│                       │                                      │
│                Agent B applies top 3                         │
│                high-priority fixes                           │
└──────────────────────────────────────────────────────────────┘
```

1. **Agent A** reads your source code and produces a structured review with prioritized findings
2. **Agent B** independently verifies each finding against the actual source code
3. They debate back and forth until they reach **consensus** (or hit the exchange limit)
4. Once agreed, **Agent B applies the top fixes** directly to your codebase
5. Repeat for as many rounds as configured

---

## Key Features / 核心特性

| Feature | Description |
|---------|-------------|
| **Multi-Round Review** | Outer loop of review cycles — each round ends with automated code fixes |
| **Exchange Debates** | Inner loop where agents argue until consensus, with smart exchange classification |
| **Any CLI Agent** | Works with Claude Code, OpenAI Codex, Gemini CLI, or any custom tool |
| **Live TUI Dashboard** | Real-time ratatui dashboard with conversation preview, agent status, timing, and scrollable logs |
| **Smart Consensus** | Keyword detection with negation handling and word boundary checks — `"I don't agree"` and `"whatnot agree"` are correctly rejected |
| **Auto-Apply Fixes** | Agent B writes the agreed top-3 fixes directly to your files after consensus |
| **Heartbeat Monitor** | 30-second progress pings so you know agents are still running |
| **Bilingual** | Full Chinese/English support in prompts, TUI, and conversation logs |

---

## Quick Start / 快速开始

### Prerequisites / 前置条件

- [Rust](https://rustup.rs/) (1.75+)
- At least one AI CLI agent / 至少一个 AI CLI 工具:
  - [Claude Code](https://claude.ai/download) — `claude`
  - [Codex CLI](https://github.com/openai/codex) — `codex`
  - Any agent that accepts a prompt as a CLI argument

### Install / 安装

```bash
git clone https://github.com/cloveric/mdtalk
cd mdtalk
cargo install --path .
```

### Run / 运行

```bash
# Review with Claude (A) + Codex (B)
# 使用 Claude 审查 + Codex 验证
mdtalk --project /path/to/your/project

# Both agents using Claude
# 两个 agent 都用 Claude
mdtalk --project . --agent-a claude --agent-b claude

# 2 rounds, 3 exchanges per round
# 2 轮审查，每轮最多 3 次讨论
mdtalk --project . --max-rounds 2 --max-exchanges 3

# Review only, no code changes
# 只讨论不改代码
mdtalk --project . --no-apply

# Use a config file / 使用配置文件
mdtalk --config mdtalk.toml

# Preview the dashboard / 预览仪表盘
mdtalk --demo
```

---

## Configuration / 配置

Create `mdtalk.toml` in your project root:

```toml
[project]
path = "."                    # Path to the project to review

[agent_a]
name = "claude"               # Display name
command = "claude"            # CLI command to invoke
timeout_secs = 900            # Per-invocation timeout (15 min)

[agent_b]
name = "codex"
command = "codex"
timeout_secs = 900

[review]
max_rounds = 1                # Review cycles (each = debate + code fix)
max_exchanges = 5             # Max A↔B exchanges per round
output_file = "conversation.md"
consensus_keywords = [
  "agree", "consensus", "LGTM", "looks good", "no further",
  "达成一致", "同意"
]

[dashboard]
refresh_rate_ms = 500
```

---

## CLI Reference / 命令行参考

```
mdtalk [OPTIONS]

Options:
  -p, --project <PATH>        Path to the project to review
  -c, --config <FILE>         Path to mdtalk.toml config file
      --agent-a <CMD>         Command for Agent A (default: claude)
      --agent-b <CMD>         Command for Agent B (default: codex)
  -m, --max-rounds <N>        Number of review rounds (default: 1)
  -e, --max-exchanges <N>     Max exchanges per round (default: 5)
      --no-dashboard          Log to stdout instead of TUI
      --no-apply              Skip code modification after consensus
      --demo                  Preview the dashboard with mock data
  -h, --help                  Print help
```

---

## Architecture / 架构

```
mdtalk/
└── src/
    ├── main.rs           # Entry point, CLI parsing, task orchestration
    ├── config.rs         # Config loading (TOML file + CLI args)
    ├── agent.rs          # Async subprocess runner with deadlock prevention
    ├── conversation.rs   # Markdown conversation file management
    ├── consensus.rs      # Keyword + negation + word-boundary consensus detection
    ├── orchestrator.rs   # Two-layer review loop with ExchangeKind state machine
    └── dashboard/
        ├── mod.rs        # TUI entry (runs on spawn_blocking thread)
        ├── app.rs        # Application state, start confirmation screen
        ├── ui.rs         # ratatui layout: status + preview + agents + logs
        └── events.rs     # Keyboard handling (Windows-compatible)
```

**Key Design Decisions / 关键设计决策:**

- **`tokio::task::spawn_blocking`** for TUI — crossterm blocks the OS thread; running on tokio's async pool would starve the orchestrator
- **Concurrent stdout/stderr reads** via `tokio::spawn` — prevents pipe buffer deadlock when agent output is large
- **`tokio::sync::watch`** for orchestrator→dashboard state; **`oneshot`** for dashboard→orchestrator start signal
- **`LineWriter`** wrapping log file — ensures log lines survive process abort
- **`ExchangeKind` enum** — cleanly classifies each exchange as `InitialReview`, `RoundReReview`, or `FollowUp` for prompt selection
- **Windows process tree kill** via `taskkill /T /F /PID` — ensures agent subprocess cleanup on timeout

---

## Real-World Results / 实际效果

### Self-Review: MDTalk reviewing its own code

| Metric | Agent A (Claude) | Agent B (Codex) |
|--------|-------------------|-----------------|
| **Time** | ~80 seconds | ~170 seconds |
| **Findings** | 13 issues across all modules | Verified all 13, added 5 new issues |
| **Consensus** | Reached in Round 1 | Applied 9 files of fixes |

**Top issues discovered / 发现的关键问题:**
- Pipe deadlock in `agent.rs` (stdout/stderr read after wait)
- Semantic bug in `append_round_header` (wrong parameter passed)
- Missing word boundary check in consensus detection
- Codex sandbox permission issue (`--full-auto` defaults to read-only)

---

## License

MIT — see [LICENSE](LICENSE)

---

<div align="center">

**MDTalk** — because the best code review is a disagreement that ends in agreement.

**MDTalk** — 最好的代码审查，是一场最终达成共识的争论。

[Star on GitHub](https://github.com/cloveric/mdtalk) · [Report a Bug](https://github.com/cloveric/mdtalk/issues) · [Request a Feature](https://github.com/cloveric/mdtalk/issues)

</div>
