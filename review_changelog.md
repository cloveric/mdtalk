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

