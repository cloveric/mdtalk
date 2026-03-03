<div align="center">

# 🤝 MDTalk

**Let two AI agents debate your code — then fix it.**

*One AI reviewing its own code rarely finds real problems. Two AIs arguing about it always do.*

[![Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-blue)](https://github.com/cloveric/mdtalk)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Status](https://img.shields.io/badge/status-active-brightgreen)](https://github.com/cloveric/mdtalk)

</div>

---

## The Problem

You've just finished a feature. You ask Claude to review the code. It says it looks great — maybe tweaks a comment or two. But the real bug is still there.

**The same AI that wrote it can't properly review it.**

The solution has always been obvious: get a second opinion from someone who wasn't involved. MDTalk automates exactly that — it puts two AI agents in a room and makes them argue until they agree on what needs fixing.

---

## How It Works

```
┌─────────────────── Round Loop (max_rounds) ───────────────────┐
│                                                                │
│  ┌──────────── Exchange Loop (max_exchanges) ─────────────┐   │
│  │                                                         │   │
│  │  Agent A  ──── reviews src/ ──────────────────────►    │   │
│  │                                         writes to ▼    │   │
│  │  Agent B  ◄─── reads conversation.md ── conversation   │   │
│  │           ──── verifies each finding ──────────────►   │   │
│  │                                         writes to ▼    │   │
│  │                 Consensus Check ◄─────── conversation   │   │
│  │                  /         \                            │   │
│  │           Agreed ✓      Disagree → next exchange        │   │
│  └─────────────────────────────────────────────────────────┘   │
│                         │                                      │
│                  Agent B applies top 3                         │
│                  high-priority fixes                           │
└────────────────────────────────────────────────────────────────┘
```

1. **Agent A** reads your source code and produces a structured review
2. **Agent B** reads the review AND the source — verifying each finding against the actual code
3. They go back and forth until they reach **consensus** (or hit the exchange limit)
4. Once agreed, **Agent B automatically applies the top fixes** to your codebase
5. Repeat for as many rounds as you need

---

## Live Dashboard

```
┌ MDTalk Dashboard ──────────────────────────────────────────────────────────┐
│ Status: Agent B (codex) responding   │  Round: 1/2  │  Exchange: 2/5       │
│ Elapsed: 00:03:42  │  q to quit, ↑↓ to scroll                              │
└────────────────────────────────────────────────────────────────────────────┘
┌ Conversation Preview ────────────────────────┐┌ Agent Status ─────────────┐
│ # Code Review: my-project                    ││ Agent A: claude  ○ waiting │
│ ## Review Session - 2026-03-03 13:30:00      ││ Agent B: codex   ● active  │
│                                              ││                           │
│ ### Round 1                                  ││ Round Times:              │
│                                              ││   Round 1: 2m 30s         │
│ #### claude - Initial Review [13:32:31]      ││   Round 2: in progress... │
│                                              ││                           │
│ ### 1. [HIGH] agent.rs:88 Potential Deadlock ││                           │
│ Reading stdout/stderr after wait() can       ││                           │
│ deadlock when pipe buffer fills up.          ││                           │
└──────────────────────────────────────────────┘└───────────────────────────┘
┌ Log ───────────────────────────────────────────────────────────────────────┐
│ [13:30:00] Session started                                                  │
│ [13:30:01] Round 1: Agent A (claude) starting review                        │
│ [13:32:31] Round 1: Agent A done (150s)                                     │
│ [13:32:31] Round 1: Agent B (codex) starting response                       │
└────────────────────────────────────────────────────────────────────────────┘
```

> Preview with `mdtalk --demo`

---

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (1.75+)
- At least one AI CLI agent:
  - [Claude Code](https://claude.ai/download) — `claude`
  - [Codex CLI](https://github.com/openai/codex) — `codex`
  - Any agent that accepts a prompt as a CLI argument

### Install

```bash
git clone https://github.com/cloveric/mdtalk
cd mdtalk
cargo install --path .
```

### Run

```bash
# Review the current directory with Claude (A) and Codex (B)
mdtalk --project /path/to/your/project

# Both agents using Claude
mdtalk --project . --agent-a claude --agent-b claude

# 2 review rounds, up to 3 exchanges per round
mdtalk --project . --max-rounds 2 --max-exchanges 3

# Review only, no code changes
mdtalk --project . --no-apply

# Use a config file
mdtalk --config mdtalk.toml
```

---

## Features

| Feature | Description |
|---------|-------------|
| 🔄 **Multi-round review** | Outer loop of review cycles; each round ends with code fixes |
| 💬 **Exchange debates** | Inner loop where agents go back and forth until consensus |
| 🤖 **Any CLI agent** | Works with Claude, Codex, Gemini CLI, or any custom tool |
| 📊 **Live TUI** | Real-time dashboard with conversation preview, timing, logs |
| ✅ **Start confirmation** | Review the config before agents start |
| 🔍 **Smart consensus** | Keyword detection with negation handling ("I don't agree" ≠ agreement) |
| 🛠️ **Auto-apply fixes** | Agent B writes the agreed fixes directly to your files |
| 🚫 **--no-apply** | Discussion-only mode, no code changes |
| 📜 **Markdown log** | Full conversation saved to `conversation.md` for review |
| ⚡ **Heartbeat** | 30s progress pings so you know agents are still running |

---

## Configuration

Create `mdtalk.toml` in your project root (or in the mdtalk directory):

```toml
[project]
path = "."                    # Path to the project to review

[agent_a]
name = "claude"               # Display name
command = "claude"            # CLI command to invoke
timeout_secs = 600            # Per-invocation timeout

[agent_b]
name = "codex"
command = "codex"
timeout_secs = 600

[review]
max_rounds = 1                # Review cycles (each = debate + code fix)
max_exchanges = 5             # Max A+B exchanges per round before giving up
output_file = "conversation.md"
consensus_keywords = [
  "agree", "consensus", "LGTM", "looks good", "no further",
  "达成一致", "同意"
]
```

---

## CLI Reference

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

## Architecture

```
mdtalk/
└── src/
    ├── main.rs           # Entry point, CLI parsing, task orchestration
    ├── config.rs         # Config loading (file + CLI args)
    ├── agent.rs          # Async subprocess runner with pipe deadlock prevention
    ├── conversation.rs   # Markdown conversation file management
    ├── consensus.rs      # Keyword + negation-aware consensus detection
    ├── orchestrator.rs   # Two-layer review loop, state machine, watch channel
    └── dashboard/
        ├── mod.rs        # TUI entry (runs on spawn_blocking thread)
        ├── app.rs        # Application state, start confirmation
        ├── ui.rs         # ratatui layout: status + preview + agents + logs
        └── events.rs     # Keyboard handling (Windows-compatible)
```

**Key design decisions:**

- `tokio::task::spawn_blocking` for the TUI — crossterm blocks the OS thread; running it on tokio's async worker pool would starve the orchestrator
- Concurrent `stdout`/`stderr` reads via `tokio::spawn` — prevents pipe buffer deadlock when agent output is large
- `tokio::sync::watch` for orchestrator→dashboard state; `oneshot` for dashboard→orchestrator start signal
- `LineWriter` wrapping log file — ensures log lines survive process abort

---

## Real-World Results

In our first self-review run (Claude reviewing MDTalk itself, Codex responding):

- **Claude (Agent A):** 104 seconds, 20 findings across all modules
- **Codex (Agent B):** 250 seconds, verified all findings against source, added 3 new issues
- **Consensus reached in Round 1**
- **Top issues found:** pipe deadlock in `agent.rs`, missing negation detection in `consensus.rs`, process tree not killed on timeout

---

## License

MIT — see [LICENSE](LICENSE)

---

<div align="center">

**MDTalk** — because the best code review is a disagreement that ends in agreement.

[⭐ Star on GitHub](https://github.com/cloveric/mdtalk) · [🐛 Report a Bug](https://github.com/cloveric/mdtalk/issues) · [💡 Request a Feature](https://github.com/cloveric/mdtalk/issues)

</div>
