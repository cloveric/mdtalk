# MDTalk - Multi-Agent Code Review System

## 项目概述

MDTalk 是一个基于 Rust 的多智能体代码审查系统。它让两个 CLI agent（如 Claude Code、Codex、Gemini CLI 等）通过一个共享的 Markdown 文件进行交互式代码审查对话，从而发现单个 agent 自检时无法发现的问题。

## 核心动机

一个程序完成后，让同一个 AI 自检往往检查不出什么问题。但让另一个 AI（如 Codex、Gemini）来审查，往往一下子就能看出问题所在或给出有价值的建议。MDTalk 自动化了这个"多视角审查"流程。

## 快速使用

```bash
# 安装
cargo install --path .

# 审查当前目录项目（默认 Agent A=claude, Agent B=codex）
mdtalk --project .

# 指定两个 agent 都用 claude
mdtalk --project . --agent-a claude --agent-b claude

# 指定配置文件
mdtalk --config mdtalk.toml

# 控制轮次和讨论次数
mdtalk --project . --max-rounds 2 --max-exchanges 3

# 无 Dashboard 模式（日志输出到 stdout）
mdtalk --project . --no-dashboard

# 跳过代码修改阶段（只讨论不改代码）
mdtalk --project . --no-apply

# 预览 Dashboard 布局（不实际运行 agent）
mdtalk --demo
```

## 架构设计

### 核心组件

1. **Orchestrator（编排器）** `src/orchestrator.rs`
   - 管理 review 对话的生命周期
   - **两层循环**：外层为轮次（rounds，每轮=达成共识+代码修改），内层为讨论（exchanges，A发言+B发言+共识检测）
   - **ExchangeKind 枚举**：`InitialReview` / `RoundReReview` / `FollowUp`，由 `classify_exchange(round, exchange)` 决定 prompt 类型
   - `should_append_round_header(exchange)` 确保每轮只写一次标题（修复了之前传 `total_exchange` 导致的语义错误）
   - 通过 `tokio::sync::watch` 向 Dashboard 推送状态
   - 通过 `tokio::sync::oneshot` 接收 Dashboard 的开始信号
   - 30 秒心跳机制，汇报 agent 运行状态
   - 通过 `tokio::sync::mpsc` 接收 Dashboard 的 apply 确认命令（手动 apply 模式）
   - 状态机：`Init → AgentAReviewing → AgentBResponding → CheckConsensus → [WaitingForApply →] ApplyChanges → Done`
   - `Phase::WaitingForApply`：手动 apply 模式下，达成共识后暂停等待用户按 Enter 确认
   - 包含单元测试验证 exchange 分类和 round header 逻辑

2. **Agent Runner（Agent 运行器）** `src/agent.rs`
   - 通过 `tokio::process::Command` 异步调用 CLI 工具
   - 支持 claude (`claude -p "prompt" --output-format text`) 和 codex (`codex exec --full-auto "prompt"`)
   - **Codex sandbox 注意**：`--full-auto` 默认只给 `read-only` sandbox，无法修改代码。实际需要 `--dangerously-bypass-approvals-and-sandbox` 才能让 Codex 在 apply 阶段写文件
   - Windows 下通过 `cmd /C` 包装 npm 安装的 CLI 工具
   - 移除 `CLAUDECODE` 环境变量防止嵌套 session 检测
   - **并发读取 stdout/stderr + wait**，避免管道缓冲区满导致的死锁
   - 支持超时控制（默认 900 秒）
   - Windows 进程树 kill（`taskkill /T /F /PID`）
   - 包含单元测试验证 CLI 参数构建

3. **Conversation（对话文件）** `src/conversation.rs`
   - 使用 `OpenOptions::append()` 追加写入
   - 格式：`### 第N轮` → `#### agent-name - Label [HH:MM:SS]` → 内容
   - 全中文标题和标签

4. **Consensus（共识检测）** `src/consensus.rs`
   - 关键词匹配 + 否定前缀检测（"don't agree" 不算共识）
   - **词边界检查**：避免 "whatnot agree" 等子串误匹配（`ends_with_negation()` 函数）
   - 支持中英文否定词（don't, wouldn't, shouldn't, 不, 未, 无法 等）
   - 可配置关键词列表
   - 7 个单元测试覆盖（含词边界测试、多空格否定测试）

5. **Dashboard（仪表盘）** `src/dashboard/`
   - ratatui + crossterm 实现的 TUI
   - **启动确认屏幕**：5 个配置字段（Agent A/B、轮次、讨论次数、自动修改开关）
   - **自动修改开关**（`auto_apply`）：设为"否"时，达成共识后暂停等待用户按 Enter 确认再执行代码修改
   - **重新开始**：会话结束后按 `r` 键可重新开始（回到启动屏），通过 `DashboardExit::Restart` + `main.rs` 循环实现
   - 状态栏（轮次+讨论进度）+ 对话预览（可滚动）+ Agent 状态面板 + 日志面板
   - 使用 `tokio::task::spawn_blocking` 运行，避免阻塞 tokio 异步线程
   - 支持 `--demo` 模式用 TestBackend 渲染预览
   - Windows `KeyEventKind::Press` 过滤，防止重复按键

### 工作流程

```
┌─────────────── 轮次循环 (max_rounds) ───────────────┐
│                                                       │
│  ┌────────── 讨论循环 (max_exchanges) ──────────┐    │
│  │                                                │    │
│  │  Agent A 审查 src/ → 写入 conversation.md     │    │
│  │                    ↓                           │    │
│  │  Agent B 读 conversation.md + src/             │    │
│  │  → 验证 A 的意见 → 追加到 conversation.md     │    │
│  │                    ↓                           │    │
│  │             检测共识关键词                      │    │
│  │              ↙         ↘                       │    │
│  │        达成共识      未达成共识 → 继续讨论     │    │
│  └────────────────────────────────────────────────┘    │
│                    ↓                                    │
│      [手动模式] 等待用户按 Enter 确认                   │
│                    ↓                                    │
│           Agent B 应用代码修改                          │
│                    ↓                                    │
│             进入下一轮                                  │
└───────────────────────────────────────────────────────┘
```

## 项目结构

```
mdtalk/
├── CLAUDE.md               # 本文件 - 项目文档
├── Cargo.toml              # Rust 2024 edition, 依赖列表
├── Cargo.lock              # 依赖锁文件
├── mdtalk.toml             # 默认配置文件
├── DEVLOG.md               # 开发日志 - 记录开发过程中的问题和解决方案
└── src/
    ├── main.rs             # 入口 + clap CLI 参数解析
    ├── config.rs           # mdtalk.toml 解析 + CLI 参数转配置
    ├── agent.rs            # Agent 子进程调用（核心）
    ├── conversation.rs     # Markdown 对话文件读写
    ├── consensus.rs        # 共识检测（含单元测试）
    ├── orchestrator.rs     # 核心编排循环 + 状态机
    └── dashboard/
        ├── mod.rs          # Dashboard 入口 + render_demo()
        ├── app.rs          # TUI 应用状态
        ├── ui.rs           # TUI 渲染布局
        └── events.rs       # TUI 键盘事件处理
```

## 技术栈

| 用途 | 库 |
|------|-----|
| 异步运行时 | `tokio` (full) |
| CLI 参数 | `clap` (derive) |
| TUI | `ratatui` + `crossterm` |
| 配置解析 | `serde` + `toml` |
| 时间 | `chrono` |
| 错误处理 | `anyhow` |
| 日志 | `tracing` + `tracing-subscriber` |

## Agent 调用细节

```rust
// Claude Code
Command::new("cmd").args(["/C", "claude", "-p", &prompt, "--output-format", "text"])

// Codex CLI（审查阶段）
Command::new("cmd").args(["/C", "codex", "exec", "--full-auto", &prompt])

// Codex CLI（需要写文件时，如 apply 阶段）
Command::new("cmd").args(["/C", "codex", "exec", "--dangerously-bypass-approvals-and-sandbox", &prompt])

// 通用 agent（直接传 prompt 作为参数）
Command::new(&command).args([&prompt])
```

**关键注意事项：**
- Windows 上 npm 安装的 CLI 是 `.cmd` 脚本，必须通过 `cmd /C` 调用
- 必须 `.env_remove("CLAUDECODE")` 否则 Claude 检测到嵌套 session 会报错
- Codex 的 `receiving-code-review` 技能会拦截 "review response" 类 prompt，需要将 prompt 表述为 "independent code review" 任务
- **Codex sandbox 陷阱**：`--full-auto` 文档说是 `workspace-write`，但实际运行时 sandbox 为 `read-only`。必须用 `--dangerously-bypass-approvals-and-sandbox` 才能让 Codex 真正修改文件

## 配置文件 (mdtalk.toml)

```toml
[project]
path = "."                   # 要审查的目标项目路径

[agent_a]
name = "claude"
command = "claude"
timeout_secs = 900

[agent_b]
name = "codex"
command = "codex"
timeout_secs = 900

[review]
max_rounds = 1               # 轮次数（每轮 = 共识 + 代码修改）
max_exchanges = 5             # 每轮最多讨论次数（A+B 来回）
consensus_keywords = ["agree", "consensus", "达成一致", "同意", "no further", "looks good", "LGTM"]
output_file = "conversation.md"

[dashboard]
refresh_rate_ms = 500
```

## 已知问题 & TODO

### 已修复
- [x] stdout/stderr 管道死锁（改为并发读取 + wait）
- [x] Windows `cmd /C` 包装 npm CLI 工具
- [x] `CLAUDECODE` 环境变量导致嵌套 session 错误
- [x] Codex `-q` 参数不存在（改为 `exec --full-auto`）
- [x] Codex prompt 被 skill 拦截（改为 "independent review" 表述）
- [x] `conversation.md` 追加写入（改用 `OpenOptions::append`）
- [x] 共识检测的否定前缀检测（支持中英文否定）
- [x] Dashboard 完成后循环的冗余检查
- [x] UTF-8 多字节字符边界 panic（`is_char_boundary()` 检查）
- [x] Windows 进程树 kill（`taskkill /T /F /PID`）
- [x] Dashboard 退出后 Orchestrator 未取消（`abort_handle()` 模式）
- [x] 日志文件创建失败时静默忽略（改为 `eprintln` 警告）
- [x] Terminal 异常退出未恢复（提取 `restore_terminal()` 保护函数）
- [x] 心跳机制：30秒间隔汇报 agent 运行状态
- [x] TUI 全中文化（界面、对话标题、日志、共识摘要）
- [x] `--no-apply` 参数跳过代码修改阶段
- [x] Apply 阶段限制只修改 3 个高优先级问题
- [x] Demo mock 数据中文化
- [x] 编译器 0 warning（清理 dead code、unused assignments）
- [x] 否定前缀扩展（haven't, wouldn't, shouldn't 等）
- [x] 滚动越界保护（scroll_down 上界检查）
- [x] 默认 agent name 与 command 一致
- [x] 两层循环：轮次（rounds）× 讨论（exchanges），支持多轮审查-修改循环
- [x] 启动确认屏幕（按 Enter 开始，显示配置摘要）
- [x] Dashboard 完成后未更新状态（watch channel sender-drop 时读取最终值）
- [x] 对话预览不再截断（保留完整内容，支持上下滚动）
- [x] Agent B prompt 中文化
- [x] Windows 按键重复（KeyEventKind::Press 过滤 + drain_buffered_events）
- [x] Dashboard 阻塞 tokio 线程导致 orchestrator 无法启动（改用 spawn_blocking）
- [x] 日志文件缓冲丢失（改用 LineWriter 确保每行立即刷新）

### 已修复（最近更新）
- [x] Codex sandbox 权限问题（`--full-auto` 实际为 `read-only`，改用 `--dangerously-bypass-approvals-and-sandbox`）
- [x] Agent B prompt 重写（Codex 只报告"已读完"不输出审查内容，改为明确要求输出完整审查文本）
- [x] Agent 超时从 300 秒增加到 900 秒
- [x] `append_round_header` 语义错误（传入 `total_exchange` 而非 `round`，导致对话标题错误）
- [x] Orchestrator exchange 分类重构（`ExchangeKind` 枚举 + `classify_exchange()` 函数）
- [x] 共识检测词边界检查（避免 "whatnot agree" 误匹配）
- [x] 新增单元测试：orchestrator（3 个）、consensus（2 个新增）、agent（1 个）
- [x] **重新开始功能**：会话结束后按 `r` 键重启回到启动屏（`DashboardExit::Restart` + main.rs loop）
- [x] **手动 apply 模式**：启动屏"自动修改"开关设为"否"，达成共识后暂停等待 Enter 确认（`Phase::WaitingForApply` + `mpsc` channel）
- [x] `MdtalkConfig` 及子结构体派生 `Clone`，支持重启循环中重复使用配置
- [x] Agent 错误信息改进：分离非零退出码和空输出的错误提示，包含 stdout/stderr 前 500 字符

### 待改进（功能增强）
- [ ] `dashboard.refresh_rate_ms` 配置项未生效（tick_rate 硬编码 100ms）
- [ ] 对话文件写入目标项目目录（应写入 sessions/ 或 mdtalk 自身目录）
- [ ] 无 session 管理（每次覆盖 conversation.md）
- [ ] Agent args 硬编码（无模板系统）
- [ ] Codex apply 阶段应使用不同于审查阶段的 sandbox 参数（当前统一使用 `--full-auto`）

### 自检发现的问题（待修复）
以下问题由 MDTalk 自检发现（claude+claude 14 条全部确认，codex+codex 7 条中 5 条成立）：
- [ ] **[高]** apply 阶段 ~50 行重复代码（手动模式和自动模式）→ 提取 `run_apply_phase()` 函数
- [ ] **[高]** 共识检测缺少转折词处理（"I agree, however..." 会被误判为共识）→ 增加 but/however/但是 检测
- [ ] **[高]** Dashboard `enable_raw_mode()` 后 `execute!`/`Terminal::new` 失败时终端不恢复 → 用 RAII guard 包裹（codex 发现）
- [ ] **[中]** 超时默认值不一致（代码 `default_timeout()` 返回 600，文档和 toml 写 900）
- [ ] **[中]** `RoundReReview` prompt 写死"代码已被修改"，但 `--no-apply`/取消/失败时代码未改（codex 发现）
- [ ] **[中]** `last_a_response`/`last_b_response` 应用 `Option<String>` 替代空字符串 + `#[allow(unused_assignments)]`
- [ ] **[中]** Windows/非 Windows 分支代码重复（agent.rs）→ 先构建 Command 再统一配置
- [ ] **[中]** 日志初始化失败时 Dashboard 模式完全没有 tracing subscriber
- [ ] **[中]** magic number 4/5 硬编码（dashboard field count）→ 定义 `const FIELD_COUNT`
- [ ] **[低]** restart 循环中 cfg 不保留上一次用户选择
- [ ] **[低]** Markdown 着色 `starts_with('#')` 过于宽泛（匹配代码中的 #include 等）
- [ ] **[低]** `has_affirmative_keyword` 和 `has_negated_keyword` 逻辑高度重复 → 提取公共函数
- [ ] **[低]** 无集成测试（可用 `echo "I agree"` 作 mock agent）
