<div align="center">

<br>

<img src="assets/banner.png" alt="MDTalk" width="720">

<br>

<p><strong>中文名：左右互博</strong></p>

<h3>Your AI says <em>"LGTM"</em>. MDTalk starts a two-agent code war and drags hidden bugs into daylight.</h3>

<p><strong>The overkill review pipeline:</strong> adversarial debate, forced consensus, and one-command fixes.</p>

<p>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Built_with-Rust-ef4a00?style=for-the-badge&logo=rust&logoColor=white" alt="Rust"></a>&nbsp;
  <a href="https://github.com/cloveric/mdtalk"><img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-0078D4?style=for-the-badge" alt="Platform"></a>&nbsp;
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-22c55e?style=for-the-badge" alt="License"></a>&nbsp;
  <a href="https://github.com/cloveric/mdtalk/stargazers"><img src="https://img.shields.io/github/stars/cloveric/mdtalk?style=for-the-badge&color=f59e0b&label=Stars" alt="Stars"></a>
</p>

<p>
  <a href="#-about">About</a> &nbsp;·&nbsp;
  <a href="#-work-interface">Work Interface</a> &nbsp;·&nbsp;
  <a href="#-quick-start">Quick Start</a> &nbsp;·&nbsp;
  <a href="#-how-it-works">How It Works</a> &nbsp;·&nbsp;
  <a href="#-configuration">Configuration</a> &nbsp;·&nbsp;
  <a href="#-architecture">Architecture</a> &nbsp;·&nbsp;
  <a href="#中文说明">中文</a>
</p>

<br>

</div>

## 🧭 About

**MDTalk (中文名：左右互博)** is an intentionally aggressive multi-agent review system.  
It is built to challenge "looks good" complacency: one agent proposes findings, the other verifies and challenges them, and the process does not stop until consensus or exhaustion.

If normal AI review is a quick glance, MDTalk is a stress test.

## 💡 The Problem

You just finished a feature. Your AI reviewer says *"looks great!"*  
Production says otherwise.

**Meanwhile, the pipe deadlock, the semantic parameter bug, and the sandbox permission hole are all still there.**

We ran MDTalk on its own codebase. Agent A (Claude) found 13 issues. Agent B (Codex) verified all 13, **then found 5 more**. They debated, agreed, and Agent B applied fixes to **9 files** — all in one command.

> **This is not a second opinion. It's a cross-examination.**

<br>

## ✨ Highlights

<img src="assets/dashboard.png" alt="Dashboard Preview" width="45%" align="right">

- 🤖 **Multi-Agent Debate** — Two AIs cross-examine each other
- 🔧 **Auto-Fix** — Agent B applies agreed fixes after consensus (High / High+Med / All)
- 🔌 **Any CLI Agent** — Claude, Codex, Gemini, or your own
- 📊 **Live TUI** — Real-time dashboard with status & logs
- 🎛️ **Parameter-Rich Control** — CLI flags + interactive start screen knobs
- 🧠 **Smart Consensus** — Negation-aware keyword matching
- 🔄 **Multi-Round** — Rounds (fix code) × Exchanges (debate)

<br clear="right">

## 🖥️ Work Interface

<p align="center">
  <img src="assets/work-interface-1.jpg" alt="MDTalk Work Interface 1" width="48%">
  <img src="assets/work-interface-2.jpg" alt="MDTalk Work Interface 2" width="48%">
</p>

<p align="center"><em>Real MDTalk working interface (start screen + runtime dashboard).</em></p>

## 🔄 How It Works

```mermaid
flowchart LR
    A["🔍 Agent A\nReview"] --> C["📄 conversation.md"]
    C --> B["✅ Agent B\nVerify"]
    B --> D{"Consensus?"}
    D -- No --> A
    D -- Yes --> F["🔧 Apply Fixes"]
    F --> R{"Next\nRound?"}
    R -- Yes --> A
    R -- No --> Done["✅ Done"]
```

> **Round** = one full debate-then-fix cycle &nbsp;|&nbsp; **Exchange** = one A↔B back-and-forth within a round

| Step | What happens |
|:---:|---|
| **1** | **Agent A** reads your source code, produces a prioritized review |
| **2** | **Agent B** independently verifies each finding against actual code |
| **3** | They debate until **consensus** or the exchange limit is hit |
| **4** | **Agent B applies** the top 3 agreed fixes to your files |
| **5** | Repeat for the next round (re-review the updated code) |

<details>
<summary><strong>📐 Rounds vs Exchanges — explained</strong></summary>
<br>

MDTalk uses a **two-layer loop**:

| Concept | Meaning | Default | Flag |
|---------|---------|:-------:|------|
| **Round** | A complete *debate + code fix* cycle. After consensus, fixes are applied, then a fresh review begins on the updated code. | 1 | `--max-rounds` |
| **Exchange** | One back-and-forth inside a round: A speaks → B responds → consensus check. | 5 | `--max-exchanges` |

**Example:** `--max-rounds 2 --max-exchanges 3`
- Up to **2 rounds** (each ending with code fixes)
- Up to **3 exchanges** per round to reach consensus
- Total: up to 6 debates, 2 rounds of code fixes

</details>

<br>

## 🚀 Quick Start

**Prerequisites:** [Rust](https://rustup.rs/) 1.75+ and at least one AI CLI — [Claude Code](https://claude.ai/download), [Codex](https://github.com/openai/codex), or any prompt-accepting CLI.

> Use `--project <path>` or `--config <path>`.  
> When using `--project`, MDTalk auto-loads `<project>/mdtalk.toml` if it exists.

**For users (stable install from a release tag):**
```bash
cargo install --git https://github.com/cloveric/mdtalk --tag <release-tag> mdtalk
```

**For contributors (run local source):**
```bash
git clone https://github.com/cloveric/mdtalk && cd mdtalk
cargo run -- --project .
```

```bash
# Claude (A) + Codex (B) review your project
mdtalk --project /path/to/your/project

# Both agents using Claude
mdtalk --project . --agent-a claude --agent-b claude

# 2 rounds × 3 exchanges
mdtalk --project . --max-rounds 2 --max-exchanges 3

# Apply all agreed issues (not only high priority)
mdtalk --project . --apply-level 3

# Discuss only, don't touch code
mdtalk --project . --no-apply

# Load from a specific config file path
mdtalk --config ./mdtalk.toml

# Preview the TUI layout
mdtalk --demo
```

If behavior looks outdated, verify your active binary and build metadata:
```bash
where mdtalk
mdtalk --version
```

<br>

## ⚙️ Configuration

Parameter precedence for dashboard mode:
- `defaults < config file < CLI flags < start screen`

In `--no-dashboard` mode:
- `defaults < config file < CLI flags`

Create `mdtalk.toml` in your project root:

```toml
[project]
path = "."

[agent_a]
name = "claude"
command = "claude"
timeout_secs = 900            # 15 min per invocation

[agent_b]
name = "codex"
command = "codex"
timeout_secs = 900

[review]
max_rounds = 1                # Debate + fix cycles
max_exchanges = 5             # A↔B exchanges per round
output_file = "conversation.md"
consensus_keywords = [
  "agree", "consensus", "LGTM", "looks good",
  "no further", "达成一致", "同意"
]
```

<details>
<summary><strong>📋 Full CLI Reference</strong></summary>

```
mdtalk [OPTIONS]

Options:
  -V, --version               Print version/build information and executable path
  -p, --project <PATH>        Project directory to review
  -c, --config <FILE>         Path to mdtalk.toml
  --agent-a <CMD>         Agent A command (default: claude)
  --agent-b <CMD>         Agent B command (default: codex)
  -m, --max-rounds <N>        Review rounds (default: 1)
  -e, --max-exchanges <N>     Exchanges per round (default: 5)
      --apply-level <1|2|3>   Apply severity: 1=High, 2=High+Medium, 3=All (default: 1)
      --no-dashboard          Log to stdout instead of TUI
      --no-apply              Skip code modification after consensus
      --demo                  Preview dashboard with mock data
  -h, --help                  Print help
```

</details>

<details>
<summary><strong>🎛️ Dashboard Start Screen Parameters</strong></summary>

When you run with the TUI dashboard, press `Enter` on the start screen to launch with your chosen runtime parameters:

| Field | Range / Values | Meaning |
|------|------|------|
| `Agent A` | `claude` / `codex` / `gemini` | Reviewer agent |
| `A Timeout` | `60..7200s` | Per-invocation timeout for Agent A |
| `Agent B` | `claude` / `codex` / `gemini` | Verifier / applier agent |
| `B Timeout` | `60..7200s` | Per-invocation timeout for Agent B |
| `Rounds` | `1..10` | Debate + apply cycles |
| `Exchanges` | `1..10` | A↔B exchanges per round |
| `Auto Apply` | `Yes` / `No` | Apply immediately or wait for manual confirm |
| `Apply Level` | `High` / `High+Med` / `All` | Fix scope after consensus |
| `Language` | `English` / `中文` | Dashboard language |
| `Branch Mode` | `Yes` / `No` | Use isolated review branch + optional merge gate |

Keyboard: `↑↓` select field, `←→` adjust value, `Enter` start.

</details>

<br>

## 🏗️ Architecture

```
src/
├── main.rs           Entry point, CLI parsing
├── config.rs         TOML config + CLI arg merging
├── agent.rs          Async subprocess runner, deadlock prevention
├── conversation.rs   Markdown conversation file I/O
├── consensus.rs      Keyword + negation + word-boundary detection
├── orchestrator.rs   Two-layer loop, ExchangeKind state machine
└── dashboard/
    ├── mod.rs        TUI entry (spawn_blocking thread)
    ├── app.rs        App state, start confirmation screen
    ├── ui.rs         ratatui layout rendering
    └── events.rs     Keyboard handling (Windows-compatible)
```

<details>
<summary><strong>🧩 Key Design Decisions</strong></summary>

| Decision | Why |
|----------|-----|
| `spawn_blocking` for TUI | crossterm blocks the OS thread — async pool would starve the orchestrator |
| Concurrent stdout/stderr | Prevents pipe buffer deadlock on large agent output |
| `watch` + `oneshot` channels | Clean state push (orchestrator→dashboard) and start signal (dashboard→orchestrator) |
| `LineWriter` for logs | Every line survives process abort |
| `ExchangeKind` enum | Classifies exchanges as `InitialReview`, `RoundReReview`, or `FollowUp` |
| `taskkill /T /F /PID` | Kills entire Windows process tree, not just the cmd.exe wrapper |

</details>

<br>

## 📊 Real-World Results

MDTalk reviewing **its own codebase** (Claude + Codex):

| | Agent A (Claude) | Agent B (Codex) |
|:---|:---|:---|
| ⏱️ **Time** | ~80s | ~170s |
| 🔍 **Findings** | 13 issues | Verified all 13, added 5 new |
| 🤝 **Consensus** | Round 1 | Applied fixes to 9 files |

**Bugs that single-agent review missed:**

> 🐛 Pipe deadlock in subprocess management
> 🐛 Semantic bug: wrong parameter passed to conversation headers
> 🐛 Word boundary false positives in consensus detection
> 🐛 Codex sandbox silently blocking file writes

<br>

## 📄 License

[MIT](LICENSE) — use it however you like.

---

<div align="center">

<br>

**One AI says "LGTM". 左右互博 says "prove it."**

**The best code review is a disagreement that ends in agreement.**

If this project saved you from a production bug, consider giving it a ⭐

<br>

<a href="https://github.com/cloveric/mdtalk/stargazers">⭐ Star this project</a> &nbsp;·&nbsp;
<a href="https://github.com/cloveric/mdtalk/issues">🐛 Report Bug</a> &nbsp;·&nbsp;
<a href="https://github.com/cloveric/mdtalk/issues">💡 Request Feature</a>

<br>
<br>

</div>

---

<details>
<summary><h2>中文说明</h2></summary>

<br>

**项目中文名：左右互博**

### 💡 问题

你刚写完代码，让 AI 自检。它说「挺好的」。

**然而管道死锁、参数语义错误、sandbox 权限漏洞全都还在那里。**

我们用 MDTalk 审查了它自己的代码库。Agent A (Claude) 发现 13 个问题，Agent B (Codex) 全部验证通过，**又额外发现了 5 个**。它们辩论、达成共识，Agent B 直接修改了 **9 个文件** — 一条命令搞定。

> **这不是“复核”级别，而是“互博”级别。**

---

### ✨ 亮点

- 🤖 **多 Agent 辩论** — 两个独立 AI 交叉检验，对照源代码验证
- 🔧 **自动修复** — 达成共识后按级别修复（高 / 高+中 / 全部）
- 🔌 **任意 CLI Agent** — Claude Code、Codex、Gemini CLI 或任何 CLI 工具
- 📊 **实时 TUI** — 基于 ratatui 的仪表盘，含对话预览、状态、日志
- 🎛️ **参数可控** — CLI 参数 + 启动页参数双层控制
- 🧠 **智能共识** — 关键词检测 + 否定前缀 + 词边界检查
- 🔄 **多轮审查** — 轮次（含代码修改）× 讨论（直到共识）

---

### 🔄 核心概念：轮次与讨论

| 概念 | 含义 | 默认值 | CLI 参数 |
|------|------|:------:|----------|
| **轮次 (Round)** | 一次完整的「辩论 + 代码修改」循环。达成共识后应用修复，然后重新审查更新后的代码。 | 1 | `--max-rounds` |
| **讨论 (Exchange)** | 轮次内的一次来回：A 发言 → B 回应 → 共识检测。 | 5 | `--max-exchanges` |

**举例：** `--max-rounds 2 --max-exchanges 3` = 最多 2 轮审查 × 每轮 3 次辩论 = 最多 6 次辩论 + 2 次代码修复。

---

### 🚀 快速开始

**前置条件：** [Rust](https://rustup.rs/) 1.75+，至少一个 AI CLI（[Claude Code](https://claude.ai/download)、[Codex](https://github.com/openai/codex) 等）。

**普通用户（推荐：按发布 tag 安装稳定版）**
```bash
cargo install --git https://github.com/cloveric/mdtalk --tag <release-tag> mdtalk
```

**开发者（本地源码运行）**
```bash
git clone https://github.com/cloveric/mdtalk && cd mdtalk
cargo run -- --project .
```

```bash
mdtalk --project /path/to/your/project          # Claude(A) + Codex(B) 审查
mdtalk --project . --agent-a claude --agent-b claude  # 双 Claude
mdtalk --project . --max-rounds 2 --max-exchanges 3  # 2轮 × 3次讨论
mdtalk --project . --apply-level 3              # 按“全部级别”执行修改
mdtalk --project . --no-apply                    # 仅讨论不改代码
mdtalk --config ./mdtalk.toml                    # 显式加载配置文件
mdtalk --demo                                    # 预览 TUI 布局
```

> 现在需显式传入 `--project` 或 `--config`。  
> 使用 `--project` 时，会自动读取该项目目录下的 `mdtalk.toml`（若存在）。

如果行为看起来不对（像在跑旧版本），先检查当前命中的可执行文件与构建信息：
```bash
where mdtalk
mdtalk --version
```

### ⚙️ 参数说明（新增）

- CLI 新增：`--apply-level <1|2|3>`（1=仅高优先级，2=高+中，3=全部）
- 启动页新增：`A Timeout`、`B Timeout`、`Auto Apply`、`Apply Level`、`Language`、`Branch Mode`
- 参数优先级（仪表盘模式）：`默认值 < 配置文件 < CLI < 启动页`

---

### 📊 实际效果

MDTalk 审查自身代码库（Claude + Codex）：

| | Agent A（Claude） | Agent B（Codex） |
|:---|:---|:---|
| ⏱️ **耗时** | ~80 秒 | ~170 秒 |
| 🔍 **发现** | 13 个问题 | 验证全部 13 个，新增 5 个 |
| 🤝 **共识** | 第 1 轮达成 | 修改了 9 个文件 |

**单 agent 自检无法发现的问题：**

> 🐛 子进程管道死锁 · 🐛 对话标题传参语义错误 · 🐛 共识检测词边界误匹配 · 🐛 Codex sandbox 静默阻止写入

</details>
