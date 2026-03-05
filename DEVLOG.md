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

## 2026-03-03: Codex Sandbox 修复 + 多 Agent 审查改进

### Codex Sandbox 权限问题（关键发现）

**问题**：Codex 在 apply 阶段（达成共识后修改代码）完全不修改任何文件。

**诊断过程**：
1. 最初以为是 prompt 问题 — Codex 回复"已完成阅读"但不输出审查内容 → 重写 Agent B prompt
2. 重写后 Codex 能正常输出审查内容，但 apply 阶段仍然不改代码
3. 检查 `mdtalk.log` 中的 Codex stderr，发现关键信息：`sandbox: read-only`
4. 尝试 `-s workspace-write` 显式覆盖 → 无效，仍然 `read-only`
5. 尝试去掉 `--full-auto` 只用 `-s workspace-write` → 无效
6. 最终使用 `--dangerously-bypass-approvals-and-sandbox` → 成功！

**根因**：Codex `--full-auto` 的文档说是 `--sandbox workspace-write`，但实际运行时 sandbox 降级为 `read-only`。可能是 Codex 配置文件中的 `trusted_projects` 或版本行为差异导致。只有 `--dangerously-bypass-approvals-and-sandbox`（对应交互模式的 `--yolo`）才能真正给 Codex 写权限。

**修复**：`agent.rs` 中 Codex 参数改为 `exec --dangerously-bypass-approvals-and-sandbox`

### Agent B Prompt 重写

**问题**：Codex 收到审查 prompt 后只输出"已完成阅读并建立上下文"，不输出实际审查分析。

**原因**：原 prompt 分两步描述任务（"请阅读文件...然后分析..."），Codex `exec --full-auto` 模式把读文件当成任务本身，执行完第一步就认为完成。

**修复**：重写 prompt 为明确的指令风格：
- 定义角色："你是一位独立的代码审查专家"
- 明确步骤：读取 → 核实 → 输出
- 强调输出要求："你必须直接输出完整的审查文本，不要只报告你读了哪些文件"

### 超时调整
- 默认 timeout 从 300 秒增加到 900 秒（15 分钟）
- Claude 审查通常 70-80 秒，Codex 验证通常 130-280 秒，300 秒不够用

### Codex 成功修改代码（首次）
使用 `--dangerously-bypass-approvals-and-sandbox` 后，Codex 在 apply 阶段成功修改了 9 个文件（+266/-104 行）：
- `orchestrator.rs`：重构 exchange 分类为 `ExchangeKind` 枚举，修复 `append_round_header` 语义错误，新增单元测试
- `consensus.rs`：新增词边界检查（避免 "whatnot" 误匹配），新增 2 个测试
- `agent.rs`：新增单元测试验证 CLI 参数构建
- `dashboard/ui.rs`：UI 布局改进
- 其他文件：小修复和格式化

编译通过，0 错误。

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

## 2026-03-05: 共识检测优化 + UI 改进 (v0.1.17 → v0.1.18)

### 共识关键词 override 误配置（v0.1.17 前）

`mdtalk.toml` 中写有 `consensus_keywords = [...]`，但列表陈旧，缺少 "成立"/"部分成立"，导致 codex 的回复无法触发共识。
**修复**：删除 toml 中的 override，改用 `config.rs` 内置的完整默认关键词列表。

### exec output 污染结论段（v0.1.17）

Codex 在结论行后继续执行 shell 命令，命令输出被 `extract_conclusion_section` 纳入结论段，导致 turning-word 误判。
**修复**：`extract_conclusion_section` 在第一个空行（`\n\n`）处截断，排除结论后的命令输出。

### 共识规则修正（v0.1.17）

发现 CLAUDE.md 和代码中，exchange 2+ 非最后一次的规则描述有误（文档说"全/部分同意均可"，实际应为"仅全部同意"）。
**修复**：`check_consensus()` 过滤掉含"部分"/"partial"的关键词，并更新 CLAUDE.md 对应表格。

### Agent B prompt 格式重构（v0.1.17）

将结论格式要求移到 prompt 末尾，用 `=== 必填结论行 ===` 分隔，强调"必须是你回复的最后一行"。
但 codex 的 `receiving-code-review` 技能文件仍然覆盖 prompt，导致 codex 用表格格式（含"结论"列）而非"结论：同意"行。

### check_b_only 二次扫描（v0.1.18）

**问题根源**：codex 被技能文件拦截后，用表格格式汇报（有"成立"关键词但无"结论："标记），最后一行是邀请语，`extract_conclusion_section` fallback 提取到这行，无关键词，共识失败。

**修复**：`check_b_only` 新增二阶段策略：
1. Primary pass：只在结论段搜索（现有逻辑）
2. Secondary pass：仅当回复中**没有任何**结论标记时，对全文做关键词扫描
   - 保护条件：若 B 明确写了 `结论：不同意`，secondary pass 被跳过

新增 `has_explicit_conclusion_marker()` helper，3 个新测试，总计 100 个测试全部通过。

### TUI 版本显示（v0.1.17-ui）

在启动屏标题和运行中状态栏显示版本号（`env!("CARGO_PKG_VERSION")`），方便区分版本。

### 鼠标滚轮和自动跟随（v0.1.17）

- 新增鼠标滚轮支持（每次滚动 3 行）
- `update_state` 记录用户是否在底部：新内容到达时自动跟随；用户手动上滚时保留位置
