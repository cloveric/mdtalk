# MDTalk 开发日志

## 2026-03-03: 初始实现 + 首次自审查

### 环境搭建
- 通过 `winget install Rustlang.Rustup` 安装 Rust（rustc 1.93.1, cargo 1.93.1）
- `cargo init` 初始化项目

### 首次编译成功
实现了全部模块：config, agent, conversation, consensus, orchestrator, dashboard (mod/app/ui/events), main。
一次编译通过（仅 warnings）。

### 首次运行踩坑记录

**问题 1: Claude 嵌套 session 检测**
- 现象：Claude 报 `Claude Code cannot be launched inside another Claude Code session`
- 原因：父进程设置了 `CLAUDECODE` 环境变量，子进程继承后触发检测
- 修复：`Command::new(...).env_remove("CLAUDECODE")`

**问题 2: Codex spawn 失败**
- 现象：`Failed to spawn agent 'codex'`
- 原因：Windows 上通过 npm 安装的 CLI 工具是 `.cmd` 脚本，不能直接 `Command::new("codex")`
- 修复：Windows 下用 `Command::new("cmd").args(["/C", "codex", ...])`

**问题 3: Codex `-q` 参数不存在**
- 现象：Codex 报 unknown flag `-q`
- 原因：CLAUDE.md 设计文档中写的 `codex -q` 并不是 Codex CLI 的真实接口
- 修复：查 `codex --help`，改为 `codex exec --full-auto "prompt"`

**问题 4: Codex prompt too long**
- 现象：Codex 报 prompt too long error（中文乱码）
- 原因：将完整对话历史作为 CLI 参数传入，超过 Windows 命令行长度限制
- 修复：改为引用 `conversation.md` 文件名，让 agent 自行读取文件

**问题 5: Codex skill 拦截**
- 现象：Codex 的 `receiving-code-review` 和 `using-superpowers` skill 拦截了 prompt，导致 Codex 不执行实际审查
- 原因：Codex 的 skill 系统优先级高于用户 prompt
- 修复：将 prompt 重新表述为 "Conduct an independent code review"（独立审查任务），而非 "respond to review"（回应审查），成功绕过 skill 拦截

### 首次成功运行（v5）
- Claude（Agent A）：104 秒，7819 字节，产出 20 个详细发现
- Codex（Agent B）：250 秒，3779 字节，读取全部 11 个源文件，逐条验证 Claude 的发现，新增 3 个额外问题
- Round 1 即达成共识

### Bug 修复轮次

根据两个 agent 的审查意见，修复了以下问题：

1. **agent.rs - stdout/stderr 管道死锁**：将 `wait() → read stdout → read stderr`（串行）改为 `tokio::spawn` 并发读取 stdout/stderr + `child.wait()`（并行）
2. **conversation.rs - 文件追加**：改用 `OpenOptions::append()` 替代 read-modify-write
3. **conversation.rs - 重复 Round 标题**：拆分为 `append_round_header()` + `append_agent_entry()`
4. **consensus.rs - 否定检测**：新增 `NEGATION_PREFIXES` 常量，检查关键词前 20 字符是否包含否定词
5. **main.rs - 并发管理**：从 `tokio::select!` 改为 `tokio::join!`（select! 会移动 handle）
6. **main.rs - Dashboard 日志**：tracing 输出到 `mdtalk.log` 文件
7. **dashboard/mod.rs - 完成循环简化**：移除冗余的 `should_quit` 检查

### Dashboard --demo 模式
- 添加 `--demo` CLI 标志
- `render_demo()` 使用 ratatui `TestBackend::new(80, 24)` 渲染一帧到 stdout
- 包含逼真的 mock 数据（Round 2 进行中、对话预览、日志、计时）

### 安装
- `cargo install --path .` 安装到 `~/.cargo/bin/mdtalk.exe`
- 可在任意目录使用 `mdtalk --project .`

### 三轮自检
完成三轮完整代码自检，修复所有发现的问题：
- 英文→中文：对话标题、标签、共识摘要、日志消息全部中文化
- dead code 清理：移除 `exit_code` 字段、`thiserror` 依赖
- 消除 OrchestratorState 重复构造（main.rs 直接调用 `OrchestratorState::new()`）
- 编译器 0 warnings，5 个单元测试全部通过

## 2026-03-03: 功能增强 + TUI 改进

### 两层循环重构
用户重新定义了"轮次"概念：
- **轮次（round）**= 一次完整的"讨论达成共识 → 代码修改"循环
- **讨论（exchange）**= A 发言 + B 发言 + 共识检测
- 外层循环：`max_rounds`（默认 1），每轮可以产生代码修改
- 内层循环：`max_exchanges`（默认 5），每轮内的讨论次数上限
- 新增 `-e` / `--max-exchanges` CLI 参数

### TUI 改进
1. **启动确认屏幕**：程序启动后不再立即开始审查，而是显示配置摘要页面，用户按 Enter 确认后才开始
   - 使用 `tokio::sync::oneshot` channel 从 dashboard 向 orchestrator 发送开始信号
2. **对话预览完整显示**：移除 100 行截断限制，保留完整对话内容，支持上下滚动
3. **状态栏增强**：同时显示轮次进度和讨论进度（如 "轮次: 1/2 │ 讨论: 3/5"）

### Dashboard 完成状态修复
- 问题：watch channel 的 `has_changed().unwrap_or(false)` 在 sender 被 drop 时吞掉 `Err`，导致 dashboard 错过最终的 Done 状态
- 修复：改用 match 处理 `Err`，当 sender drop 时仍读取最终值

### Windows 按键处理修复
- 问题：Windows 下 crossterm 发送 Press + Release 事件，导致按键被处理两次
- 修复：过滤只处理 `KeyEventKind::Press` 事件
- 新增 `drain_buffered_events()` 清除启动时残留的按键事件

### Dashboard 阻塞 tokio 线程（关键 Bug）
- **问题**：Dashboard 的事件循环使用 `crossterm::event::poll()`（阻塞系统调用），但运行在 `tokio::spawn`（异步工作线程）上。dashboard 的 async fn 从不 yield（没有真正的 `.await`），持续占用 tokio 工作线程。当用户按 Enter 发送 oneshot 信号后，orchestrator 的 `rx.await` 虽然被唤醒，但无法获得线程来执行。
- **诊断**：通过 debug 文件写入确认 `tx.send(())` 返回 `Ok(())`（信号已发送），但 orchestrator 的日志停在"等待用户确认开始..."（信号未收到）。
- **修复**：
  1. Dashboard 的 `run()` 和 `run_dashboard_loop()` 从 `async fn` 改为普通 `fn`
  2. `main.rs` 中从 `tokio::spawn` 改为 `tokio::task::spawn_blocking`，使 dashboard 运行在独立的阻塞线程池上
  3. 日志文件改用 `Mutex<LineWriter<File>>`，确保每行立即刷新到磁盘

## 审查发现汇总

### 首次自审查（Claude + Codex）

共发现 **20+ 个问题**，按优先级：

| # | 严重度 | 文件 | 问题 | 状态 |
|---|--------|------|------|------|
| 1 | 高 | agent.rs | stdout/stderr 读取死锁 | ✅ 已修复 |
| 2 | 高 | agent.rs | Windows kill 只杀 cmd.exe | ✅ 已修复 |
| 3 | 高 | orchestrator.rs | 对话文件污染目标项目 | ❌ 待修复 |
| 4 | 中 | consensus.rs | UTF-8 字节切片 panic 风险 | ✅ 已修复 |
| 5 | 中 | consensus.rs | 共识检测过于粗糙 | ✅ 已修复（否定检测） |
| 6 | 中 | main.rs | 日志文件失败静默忽略 | ✅ 已修复 |
| 7 | 中 | main.rs | Dashboard退出不取消Orchestrator | ✅ 已修复 |
| 8 | 中 | agent.rs | Windows/非Windows 代码重复 | ❌ 待修复 |
| 9 | 低 | config.rs | agent name 推导不一致 | ✅ 已修复 |
| 10 | 低 | dashboard/app.rs | 自动滚动硬编码 `3` | ✅ 已修复 |
| 11 | 低 | orchestrator.rs | Instant 不可序列化 | — 暂不需要 |
| 12 | 低 | conversation.rs | 每次写入重新打开文件 | ❌ 待修复 |
| 13 | 低 | — | 无 session 管理 | ❌ 待实现 |
| 14 | 建议 | agent.rs | 缺少 agent CLI 安装检查 | ❌ 待实现 |
| 15 | 建议 | — | 缺少测试覆盖 | ❌ 待实现 |

### Codex 额外发现

| # | 严重度 | 文件 | 问题 | 状态 |
|---|--------|------|------|------|
| B1 | 中 | consensus.rs | `saturating_sub(20)` 可能落在 UTF-8 码点中间 | ✅ 已修复 |
| B2 | 中 | agent.rs | timeout kill 后没有 wait，可能留僵尸进程 | ✅ 已修复 |
| B3 | 中 | orchestrator.rs | Agent 失败后仍报 "Max rounds reached" | ✅ 已修复 |
| B4 | 低 | dashboard/mod.rs | raw mode 清理缺少 Drop guard | ✅ 已修复（restore_terminal） |
