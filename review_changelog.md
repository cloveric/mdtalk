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

