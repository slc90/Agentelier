# 为标题栏提供 Codex 账号 5 小时与周额度

## 目标

让 A 区显示当前 Codex 账号的 5 小时额度和周额度，并显示剩余比例与重置时间。额度状态由 Agent Account 提供，不从某个 Session 的 token usage 推算。

## 数据来源与显示语义

参考 Codex CLI `/status` 的账号 Rate Limit Snapshot。服务端窗口数据包含 `used_percent`、`window_duration_mins` 和 `resets_at`。根据窗口时长识别 5 小时与周窗口，不能永久假设 `primary` 一定是 5 小时、`secondary` 一定是周额度。剩余比例为 `100 - used_percent`。服务端没有提供的窗口不显示为零额度，而是显示不可用。

参考：
- [Rate Limit Snapshot 展示映射](https://github.com/openai/codex/blob/main/codex-rs/tui/src/status/rate_limits.rs)
- [Account Rate Limits 查询处理](https://github.com/openai/codex/blob/main/codex-rs/app-server/src/request_processors/account_processor.rs)
- [Rate Limit 响应解析](https://github.com/openai/codex/blob/main/codex-rs/codex-api/src/rate_limits.rs)

## Agent 与 Bridge 职责

当前 `agent/account` 负责账号身份和凭据分发，但尚无可供 UI 使用的额度快照。本增量扩展账号状态流程：

- Account 使用当前已确认的 Codex 身份与凭据请求账号额度快照。
- 快照携带账号身份、获取时间、窗口使用比例、窗口时长和重置时间。
- 账号切换、登出或凭据代次变化时，旧请求结果不能覆盖新账号状态。
- `bridge/account` 只投影 UI 需要的安全字段，不暴露 Token 或原始认证数据。
- `agent_host` 收到账号状态更新后唤醒 UI。

A 区上下文占用仍由 Conversation 的 `ContextState` 提供，与账号额度分开显示。

## 工程边界

- Agent 业务事实归 `crates/agent`；UI 不维护第二份 Session、任务或资源目录权威状态。
- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。通过 Cargo 检查、实际应用运行、日志、存储记录和必要的调试观察验证。
- 只有涉及可观察 GUI 行为时才执行 BRP；BRP 不等价的 OS-level 行为使用真实 Windows 窗口验证。
## 完成条件与验收

1. 已登录 Codex 账号时，Account 能获取账号 Rate Limit Snapshot 并向 UI 发布安全状态。
2. 5 小时与周额度通过窗口时长识别；当窗口顺序或字段位置变化时仍能正确标记。
3. A 区显示两种额度的剩余百分比和重置时间。
4. `used_percent` 缺失、窗口不存在或服务端返回不可用时，显示不可用状态，不显示 0% 或推测值。
5. 额度刷新失败时保留上次有效快照并标记过期/不可用，不将旧值伪装为实时数据。
6. 登录完成后显示新账号额度；登出后清除旧账号额度。
7. 账号在额度请求过程中发生切换或登出时，旧请求迟到结果被身份代次检查拒绝。
8. Account 额度快照不与某个 Session 绑定；切换 C 区 Session 不改变账号额度。
9. 上下文占用继续来自当前 Session 的 `ContextState`，并随 C 切换；账号额度与上下文窗口不混为一项。
10. 估算中的上下文尾部有明确估算标识；服务端 usage 锚点与模型窗口上限来自现有 Conversation / LLM Model 数据。
11. 日志和 UI 安全投影不包含 Access Token、Refresh Token 或凭据内容。
12. 真实应用中验证登录后刷新、账号状态更新和刷新失败；不为验收主动退出或重新登录现有账号，除非用户明确授权。
