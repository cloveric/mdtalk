# Changelog

All notable changes to MDTalk are documented in this file.

## [0.1.2] - 2026-03-03

### Added
- **交互式启动屏幕**: 启动时可直接调整 Agent A/B、轮次、讨论次数，用 ↑↓ 选择字段，←→ 调整值，Enter 确认开始 (`fa1bc06`)
- Agent 预设列表: `claude`, `codex`, `gemini`，可在启动屏幕中循环切换
- Agent B 代码修改阶段的输出写入 `conversation.md`，在 TUI 对话预览中可查看修改内容 (`30574bd`)

### Changed
- Claude 全程使用 `--dangerously-skip-permissions`，无需交互确认即可读写文件 (`d86402c`)
- Codex 从 `--full-auto`（实际为 read-only sandbox）切换为 `--dangerously-bypass-approvals-and-sandbox` (`d86402c`)
- 启动信号从 `oneshot::Sender<()>` 改为 `oneshot::Sender<StartConfig>`，支持传递用户选择的配置

## [0.1.1] - 2026-03-03

### Fixed
- Codex sandbox 权限问题: `--full-auto` 实际为 `read-only`，无法在 apply 阶段写文件 (`df41bed`)
- Agent B prompt 重写: Codex 只报告"已读完文件"不输出审查内容，改为明确要求输出完整审查文本 (`df41bed`)
- `append_round_header` 语义错误: 传入 `total_exchange` 而非 `round`，导致对话标题错误 (`df41bed`)
- 共识检测词边界检查: 避免 "whatnot agree" 等子串误匹配 (`df41bed`)

### Changed
- Agent 超时从 300 秒增加到 900 秒 (`df41bed`)
- Orchestrator exchange 分类重构: 新增 `ExchangeKind` 枚举 + `classify_exchange()` 函数 (`df41bed`)

### Added
- 新增单元测试: orchestrator (3)、consensus (2)、agent (1) (`df41bed`)

## [0.1.0] - 2026-03-03

### Added
- 核心多智能体代码审查系统，支持两个 CLI agent 通过 Markdown 对话文件进行交互式代码审查 (`96584f9`)
- 两层循环架构: 外层轮次 (rounds) × 内层讨论 (exchanges)
- Agent Runner: 异步调用 CLI 工具 (claude, codex)，支持 Windows `cmd /C` 包装
- Conversation: Markdown 对话文件追加写入，全中文标题和标签
- Consensus: 关键词匹配共识检测，支持中英文否定前缀检测
- TUI Dashboard: ratatui + crossterm 实现，状态栏 + 对话预览 + Agent 状态 + 日志面板
- 启动确认屏幕: 显示配置摘要，按 Enter 开始
- `--demo` 模式: 用 TestBackend 渲染预览
- `--no-apply` 参数: 跳过代码修改阶段
- `--no-dashboard` 模式: 日志输出到 stdout
- TOML 配置文件支持 (`mdtalk.toml`)
- CLI 参数: `--project`, `--config`, `--agent-a`, `--agent-b`, `--max-rounds`, `--max-exchanges`
- 30 秒心跳机制，汇报 agent 运行状态
- Windows 进程树 kill (`taskkill /T /F /PID`)
- stdout/stderr 并发读取，避免管道缓冲区死锁
- `CLAUDECODE` 环境变量移除，防止嵌套 session 检测
- Dashboard 完成后 Orchestrator 自动取消 (`abort_handle()`)
- 日志文件 `LineWriter` 确保每行立即刷新
- Terminal 异常退出恢复 (`restore_terminal()`)
- Windows `KeyEventKind::Press` 过滤 + `drain_buffered_events()` 防止按键重复
- README (中英双语) + MIT License (`d880f11`)
