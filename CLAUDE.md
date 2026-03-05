# MDTalk - Multi-Agent Code Review System

让两个 CLI agent（claude、codex 等）通过共享 Markdown 文件互相审查代码，发现单个 AI 自检时难以发现的问题。

详细开发历史见 [DEVLOG.md](./DEVLOG.md)。

---

## 架构设计

### 核心流程

两层循环：**轮次（rounds）** × **讨论（exchanges）**

- 一次**讨论** = Agent A 发言 + Agent B 发言 + 共识检测
- 一**轮** = 最多 N 次讨论直到共识，然后 B 修代码

关键文件：
- `src/orchestrator.rs` — 编排器，状态机，两层循环
- `src/consensus.rs` — 共识检测（关键词 + 否定词 + 转折词）
- `src/agent.rs` — CLI 子进程调用
- `src/dashboard/` — ratatui TUI

### 共识判断规则（代码实现以此为准）

| 情况 | 判断逻辑 |
|------|---------|
| exchange 1，max_exchanges = 1 | 只看 B，全部或部分同意都算 |
| exchange 1，max_exchanges > 1 | 只看 B，仅全部同意；部分/不同意继续 |
| exchange 2+ 非最后一次 | 双方都需全部同意；部分同意→继续 |
| 最后一次（exchange == max_exchanges） | 只看 B，全部或部分同意都算 |

B 的结论行格式（必须是最后一行）：
- `结论：同意` / `CONCLUSION: I agree` → 完全共识，B 修全部问题
- `结论：部分同意` / `CONCLUSION: partially agree` → 部分共识，B 只修双方认可的问题
- `结论：不同意` / `CONCLUSION: I disagree` → 无共识，不修代码

**注意**：`check_b_only` 有二次扫描兜底——当 B 完全没写结论标记时（如被技能文件拦截改用表格格式），对全文做关键词扫描。若 B 明确写了 `结论：不同意` 则跳过二次扫描。

---

## Agent 调用 Gotchas

- **Windows**：npm 安装的 CLI 是 `.cmd` 脚本，必须通过 `cmd /C` 调用
- **CLAUDECODE 环境变量**：必须 `.env_remove("CLAUDECODE")`，否则 Claude 检测到嵌套 session 报错
- **Codex skill 拦截**：`receiving-code-review` 技能会拦截 prompt，需表述为 "independent code review" 任务
- **Codex sandbox**：`--full-auto` 实际运行时 sandbox 降为 `read-only`，apply 阶段必须用 `--dangerously-bypass-approvals-and-sandbox`

---

## TODO

### 待改进（功能）
- [ ] `dashboard.refresh_rate_ms` 配置项未生效（tick_rate 硬编码 100ms）
- [ ] 对话文件写入目标项目目录（应写入 mdtalk 自身目录或 sessions/）
- [ ] 无 session 管理（每次覆盖 conversation.md）

### 自检发现的待修复
- [ ] **[中]** `RoundReReview` prompt 写死"代码已被修改"，但 `--no-apply`/取消/失败时代码未改
- [ ] **[中]** `last_a_response`/`last_b_response` 用空字符串代替 `Option<String>`
- [ ] **[中]** 日志初始化失败时 Dashboard 模式没有 tracing subscriber
- [ ] **[低]** restart 循环中不保留上一次用户在启动屏的选择
- [ ] **[低]** Markdown 着色 `starts_with('#')` 过于宽泛（匹配 #include 等）
- [ ] **[低]** 无集成测试（可用 `echo "I agree"` 作 mock agent）
