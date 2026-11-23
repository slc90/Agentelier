# 让后台任务跨 Session 持续运行并提供全局状态投影

## 目标

让多个 Session 的后台任务在用户切换当前聊天时继续运行，并向 UI 提供跨 Session 的安全任务状态快照。E 区消费该快照，不创建第二套任务数据库。

## 当前任务事实与需要改变的边界

Conversation 已有稳定 Task ID、来源 Session、Turn、调用身份、任务状态和结果交付状态；Session Store 持久化执行与结果事实。Runtime 管理后台任务和进程回收。

当前 `runtime/tool_jobs` 的 `SessionTools` 资源按单个活动 Session 保存。C 区允许切换 Session，而 E 需要同时观察全部 Session，因此 Runtime 需要把仍在运行的后台资源按 Session ID 保留，而不是在切换时丢弃前一个 Session 的工具工作者、进程观察器或结果通道。

前台模型 Turn 仍遵循现有 Conversation 的执行边界；后台工具任务的生命周期与当前 UI 选择分离。切换 Session 只切换前台会话展示，不停止其他 Session 的后台工作。

## 全局任务投影

Agent 侧提供全局任务快照，至少包含稳定 Task ID、所属 Session ID 与可显示标题、Agent 提供的安全任务名称、当前状态、结果保存/交付状态，以及是否进入明确的用户操作等待状态。

任务名称必须由 Agent 侧形成，不能让 UI 从原始参数、工具输出或命令字符串猜测。安全投影不包含来源 Turn、工具来源、参数摘要、详细输出、凭据或附件字节。

E 的可见状态包括等待执行、运行中、等待资源、取消中、失败和已保存待处理结果。成功完成且结果已交付的任务从 E 的当前可见列表移除，但 Session Store 中的历史仍保留。

## 通知事件

Agent 在任务状态变化时发布明确的通知类别：成功终态、失败终态、需要用户操作。普通运行、排队、资源等待和取消过程不产生通知事件。需要用户操作必须是明确等待用户审批、确认或补充输入的状态。

任务执行成功、结果保存和结果交付保持不同事实。成功通知由 Agent 的成功终态产生；E 的移除条件仍要求成功且结果已交付。任务取消只在实际终态确认后更新。

## Agent / Bridge 影响

- `crates/agent/conversation`：保持每个 Session 的任务身份和状态事实，提供安全显示名称及状态。
- `crates/agent/runtime`：将后台 `SessionTools`、进程和结果工作者按 Session ID 管理；聚合跨 Session 快照并发出通知事件。
- `crates/agent/session_store`：保留每个 Session 的权威任务、进程和交付历史。
- `crates/agent/turn_runner`：仍只推进当前 Turn，不接管跨 Session 任务所有权。
- `crates/bridge/conversation`：投影全局任务快照与通知类别。
- `crates/bridge/connection` / `agent_host`：在后台状态变化时唤醒 UI。

## 工程边界

- Agent 业务事实归 `crates/agent`；UI 不维护第二份 Session、任务或资源目录权威状态。
- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。通过 Cargo 检查、实际应用运行、日志、存储记录和必要的调试观察验证。
## 完成条件与验收

1. 在 Session A 启动后台任务后切换到 Session B，A 的后台工作继续运行。
2. 在 A、B 两个 Session 同时存在后台任务时，Runtime 保留两边各自的执行资源、进程观察和结果通道。
3. 切换当前 Session 不会取消、重启或重复派发其他 Session 的任务。
4. 每个任务的 Task ID、执行实例和所属 Session 稳定；迟到事件不能改写另一 Session 的任务。
5. 全局快照包含所有 Session 中需要关注的任务，不只包含当前 Conversation。
6. 任务名称来自 Agent 安全投影；Bridge 不暴露原始参数、完整输出、凭据或附件字节。
7. 执行成功、结果保存、结果交付和进程退出分别记录；工具返回不等于外部进程已退出。
8. 只有成功且已交付的任务从全局可见列表移除；成功但待交付的任务仍可见。
9. 失败任务和已保存待处理结果保持可见，直到 Agent 侧任务生命周期将其更新或移除。
10. 需要用户操作的状态有明确标识；普通资源等待不会被映射为用户操作。
11. 通知事件只在成功终态、失败终态和明确需要用户操作时产生；普通运行、排队、资源等待、取消请求不产生通知。
12. 同一 Task ID 的重复状态投影幂等；迟到旧修订不能让终态任务恢复为运行中。
13. 应用重启后，持久任务历史可以恢复；旧进程句柄不会被伪装成当前可控制的运行进程。
14. 没有键鼠输入时，后台任务状态仍能通过 Agent Host / Connection 唤醒 UI。
15. 对进程仍运行、结果保存失败、交付失败、取消与完成交错、Session 关闭和应用退出进行真实运行观察。
16. 不在 UI 创建第二个任务账本或直接访问 Runtime 内部句柄。
