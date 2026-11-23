# 为记忆管理窗口建立每次启动唯一的专用 Session

## 目标

让记忆管理窗口使用专用 Session，而不是借用 C 区当前选中的聊天。每次程序运行期间只有一个活动记忆管理 Session；下一次启动创建新的 Session。

## 生命周期

专用 Session 按需创建：当前程序第一次打开记忆管理窗口时创建，之后在本次进程内复用。关闭和重新打开窗口不会创建第二个 Session，也不会改变 C 区当前选中的 Session。

该 Session 有独立 Session ID、Turn、消息历史和草稿；不属于任何 Project，也不显示在 C 区的 Project / 单独聊天列表；通过 Memory 能力完成记忆查询与显式修订。

下一次程序启动时创建新的记忆管理 Session，不恢复上一次运行期间的 Session 作为当前活动记忆会话。历史记录仍由 Session Store 按内部会话身份保留，用于来源追踪；它们不进入用户可见 Session 导航。

## Agent 领域影响

`crates/agent/conversation` 需要区分普通用户 Session 与内部记忆管理 Session。`crates/agent/session_store` 保存内部 Session 身份和历史，但 Archive Catalog 排除内部 Session。`crates/agent/runtime` 在当前进程内持有唯一记忆 Session 的身份并路由其请求。Memory 修订仍使用真实用户原话、Session ID、Turn 和记忆纪元。

专用 Session 不建立第二个 Runtime。它复用现有 Agent Runtime 和 Conversation 的生命周期与执行身份门控。

## 工程边界

- Agent 业务事实归 `crates/agent`；UI 不维护第二份 Session、任务或资源目录权威状态。
- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。通过 Cargo 检查、实际应用运行、日志、存储记录和必要的调试观察验证。
## 完成条件与验收

1. 当前进程第一次打开记忆窗口时创建专用 Session。
2. 当前进程重复打开、关闭和重开记忆窗口时复用同一个 Session ID。
3. 记忆窗口的输入、消息历史和草稿不影响 C 区当前选中的普通 Session。
4. 记忆专用 Session 不显示在 Project / 单独聊天列表中。
5. 专用 Session 不属于 Project，不继承当前 Project 的目录归属。
6. Memory 修订请求携带真实用户语句、专用 Session ID、Turn ID 和记忆纪元，现有授权校验正常工作。
7. 专用 Session 的消息与 Turn 使用现有 Conversation / Runtime / Session Store 机制，不创建第二套聊天或执行器。
8. 程序重启后创建新的专用 Session ID，不恢复上一进程的 Session 为当前活动记忆会话。
9. 上一进程的内部 Session 历史仍按 Session Store 保留，但不出现在用户可见导航中。
10. 普通 Session 的列表、打开、删除和 Project 归属不受内部 Session 影响。
11. 应用启动后从未打开记忆窗口时，不创建不必要的专用 Session。
12. 对 Session 创建失败、存档失败、Runtime 关闭和重复打开进行真实运行观察。
