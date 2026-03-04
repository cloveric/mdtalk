# MDTalk 代码修改记录

## 第1轮 代码修改 - 2026-03-03 23:53:25

All 3 fixes complete.13 tests pass, 0 clippy warnings. Summary of changes:

**Fix #2 — `src/agent.rs`：非零退出码处理**
- 之前：非零退出码 + 有 stdout 输出 → 被当作成功（鉴权失败/限流等错误会被当作正常审查文本）
- 修复：非零退出码一律报错，同时在错误信息中包含 stdout 前 500 字符供调试。另外单独检查 stdout 为空的情况

**Fix #4 — `src/main.rs` + `src/config.rs`：参数下界校验**
- `main.rs`：clap 参数加 `value_parser = clap::value_parser!(u32).range(1..)`，CLI 层直接拒绝 0 值
- `config.rs`：`load()` 后调用 `validate()` 校验 toml 配置中的 `max_rounds` 和 `max_exchanges >= 1`

**Fix #1 — `src/consensus.rs`：混合信号误判**
- 新增 `has_negated_keyword()` 函数检测否定匹配
- 新增 `agent_shows_consensus()` 函数：同一条回复中既有肯定又有否定关键词时视为"混合信号"，不计入共识
- 新增 2 个单元测试覆盖中英文混合信号场景


---

## 第1轮 代码修改 - 2026-03-04 02:29:59

已按 `conversation.md` 的共识，只修复了最重要的 3 个高优先级问题：

1. 退出时编排器与子进程清理不可靠  
- 在 [main.rs:125](/C:/Users/hangw/mdtalk/src/main.rs:125) 增加了 `Shutdown` 控制信号发送与 3 秒优雅等待；超时再 `abort`（[main.rs:152](/C:/Users/hangw/mdtalk/src/main.rs:152), [main.rs:157](/C:/Users/hangw/mdtalk/src/main.rs:157), [main.rs:164](/C:/Users/hangw/mdtalk/src/main.rs:164)）。  
- 在 [orchestrator.rs:107](/C:/Users/hangw/mdtalk/src/orchestrator.rs:107) 新增 `OrchestratorCommand::Shutdown`，并在主循环中消费该命令提前安全退出（[orchestrator.rs:112](/C:/Users/hangw/mdtalk/src/orchestrator.rs:112), [orchestrator.rs:233](/C:/Users/hangw/mdtalk/src/orchestrator.rs:233), [orchestrator.rs:251](/C:/Users/hangw/mdtalk/src/orchestrator.rs:251)）。  
- 在 [agent.rs:67](/C:/Users/hangw/mdtalk/src/agent.rs:67) 和 [agent.rs:77](/C:/Users/hangw/mdtalk/src/agent.rs:77) 给子进程命令启用了 `kill_on_drop(true)`，避免任务被取消时遗留进程。  

2. Agent 执行失败被吞掉，最终可能返回 `Ok(())`  
- 在 [orchestrator.rs:239](/C:/Users/hangw/mdtalk/src/orchestrator.rs:239) 增加 `execution_error` 聚合。  
- Agent A/B 失败时不再仅 `break`，而是记录并保存错误（[orchestrator.rs:324](/C:/Users/hangw/mdtalk/src/orchestrator.rs:324), [orchestrator.rs:377](/C:/Users/hangw/mdtalk/src/orchestrator.rs:377)）。  
- 轮次结束后若存在执行错误，直接返回 `Err`（[orchestrator.rs:416](/C:/Users/hangw/mdtalk/src/orchestrator.rs:416)）。  

3. apply 阶段失败只记日志继续，导致“已应用”语义不可信  
- apply 时 Agent B 失败现在直接返回 `Err`（[orchestrator.rs:533](/C:/Users/hangw/mdtalk/src/orchestrator.rs:533)）。  
- `review_changelog.md` 写入失败现在也直接返回 `Err`（[orchestrator.rs:517](/C:/Users/hangw/mdtalk/src/orchestrator.rs:517)）。  

新增回归测试（先红后绿）：
- [orchestrator.rs:695](/C:/Users/hangw/mdtalk/src/orchestrator.rs:695)：`returns_err_when_agent_a_discussion_fails`  
- [orchestrator.rs:711](/C:/Users/hangw/mdtalk/src/orchestrator.rs:711)：`returns_err_when_apply_phase_fails`  

验证结果：
- 已执行 `cargo test`，`15 passed, 0 failed`。


---

## 第1轮 代码修改 - 2026-03-04 03:59:38

已根据 `conversation.md` 里达成一致的高优先级问题完成修复（中低优先级未处理）：

1. `branch_mode` 建分支失败时现在会立即中止，不再继续在当前分支改代码。  
见 [orchestrator.rs:720](/C:/Users/hangw/mdtalk/src/orchestrator.rs:720)、[orchestrator.rs:734](/C:/Users/hangw/mdtalk/src/orchestrator.rs:734)。

2. 共识检测修复了否定语义误判（`no consensus` / `not in agreement` 不再被当成达成共识）。  
见 [consensus.rs:10](/C:/Users/hangw/mdtalk/src/consensus.rs:10)、[consensus.rs:12](/C:/Users/hangw/mdtalk/src/consensus.rs:12)。

3. `git_commit_all` 现在会检查 `git add` / `git diff --cached --quiet` / `git commit` 的退出状态，并区分“已提交”与“无变更可提交”，不再误报提交成功。  
见 [orchestrator.rs:51](/C:/Users/hangw/mdtalk/src/orchestrator.rs:51)、[orchestrator.rs:68](/C:/Users/hangw/mdtalk/src/orchestrator.rs:68)、[orchestrator.rs:85](/C:/Users/hangw/mdtalk/src/orchestrator.rs:85)、[orchestrator.rs:102](/C:/Users/hangw/mdtalk/src/orchestrator.rs:102)、[orchestrator.rs:839](/C:/Users/hangw/mdtalk/src/orchestrator.rs:839)。

新增回归测试：
- [orchestrator.rs:1095](/C:/Users/hangw/mdtalk/src/orchestrator.rs:1095)
- [orchestrator.rs:1115](/C:/Users/hangw/mdtalk/src/orchestrator.rs:1115)
- [consensus.rs:252](/C:/Users/hangw/mdtalk/src/consensus.rs:252)
- [consensus.rs:259](/C:/Users/hangw/mdtalk/src/consensus.rs:259)

验证结果：
- `cargo test`：20/20 通过。  

附注：工作区还有一个未跟踪文件 `demo_output.txt`，我未改动它。


---

## 第1轮 代码修改 - 2026-03-04 19:45:11

已按 `conversation.md` 的共识结论完成代码修复，并已直接改动源码。

已落地的修复点（对应“成立/可执行的部分成立”）：

- `RoundReReview` prompt 改为按上一轮是否真的执行了 apply 动态生成，避免误导  
  [orchestrator.rs#L296](/C:/Users/hangw/mdtalk/src/orchestrator.rs#L296) [orchestrator.rs#L715](/C:/Users/hangw/mdtalk/src/orchestrator.rs#L715) [orchestrator.rs#L1179](/C:/Users/hangw/mdtalk/src/orchestrator.rs#L1179)
- 默认超时从 `600` 对齐为 `900`，并同步测试断言  
  [config.rs#L68](/C:/Users/hangw/mdtalk/src/config.rs#L68) [config.rs#L429](/C:/Users/hangw/mdtalk/src/config.rs#L429)
- 去掉共识检测阶段的冗余绑定，且不再依赖“空字符串占位”的 `last_*_response` 写法  
  [orchestrator.rs#L790](/C:/Users/hangw/mdtalk/src/orchestrator.rs#L790) [orchestrator.rs#L867](/C:/Users/hangw/mdtalk/src/orchestrator.rs#L867) [orchestrator.rs#L935](/C:/Users/hangw/mdtalk/src/orchestrator.rs#L935)
- `Phase::Display` 改为中立英文文本（不再强制中文）  
  [orchestrator.rs#L498](/C:/Users/hangw/mdtalk/src/orchestrator.rs#L498)
- `dashboard.refresh_rate_ms` 现已真正驱动 dashboard tick 频率  
  [dashboard/mod.rs#L46](/C:/Users/hangw/mdtalk/src/dashboard/mod.rs#L46) [dashboard/mod.rs#L82](/C:/Users/hangw/mdtalk/src/dashboard/mod.rs#L82) [main.rs#L171](/C:/Users/hangw/mdtalk/src/main.rs#L171)
- 滚动上界改为考虑可视行数，修复过滚；同时同步 `state.language` 到 UI 渲染语言  
  [dashboard/app.rs#L16](/C:/Users/hangw/mdtalk/src/dashboard/app.rs#L16) [dashboard/app.rs#L236](/C:/Users/hangw/mdtalk/src/dashboard/app.rs#L236) [dashboard/ui.rs#L170](/C:/Users/hangw/mdtalk/src/dashboard/ui.rs#L170)
- Markdown 标题高亮从 `starts_with('#')` 收紧为 `# ` / `## `，避免误匹配代码行  
  [dashboard/ui.rs#L360](/C:/Users/hangw/mdtalk/src/dashboard/ui.rs#L360)
- `agent.rs` Windows/Unix 启动分支去重，保留平台差异配置点  
  [agent.rs#L79](/C:/Users/hangw/mdtalk/src/agent.rs#L79) [agent.rs#L100](/C:/Users/hangw/mdtalk/src/agent.rs#L100)
- dashboard 模式日志文件创建失败时，降级到 `stderr` subscriber（不再静默丢日志）  
  [main.rs#L136](/C:/Users/hangw/mdtalk/src/main.rs#L136)
- `Conversation` append 改为复用已打开句柄（`Mutex<Option<File>>`），避免每次 reopen  
  [conversation.rs#L14](/C:/Users/hangw/mdtalk/src/conversation.rs#L14) [conversation.rs#L26](/C:/Users/hangw/mdtalk/src/conversation.rs#L26)
- 测试目录清理改为 RAII guard，避免断言失败时泄漏临时目录（`orchestrator/config/conversation`）  
  [orchestrator.rs#L1333](/C:/Users/hangw/mdtalk/src/orchestrator.rs#L1333) [config.rs#L314](/C:/Users/hangw/mdtalk/src/config.rs#L314) [conversation.rs#L183](/C:/Users/hangw/mdtalk/src/conversation.rs#L183)

验证结果：

- 已执行 `cargo test`，`42 passed, 0 failed`。

说明：

- `demo_output.txt` 仍为未跟踪文件，未改动。


---

