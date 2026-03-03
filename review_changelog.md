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

