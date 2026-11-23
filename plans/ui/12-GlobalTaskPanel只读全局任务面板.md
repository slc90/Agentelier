# 交付按 Session 分组的只读全局任务面板

## 目标

完成 E 区全局任务面板，展示所有 Session 的任务并按 Session 分组。面板只展示任务名称和状态，不提供任何任务操作。

## 展示规则

当前 `crates/ui/chat_ui/src/task_panel.rs` 是绑定单个当前 Session 的任务控制面板，包含任务操作和过程信息。本增量将它替换为 E 区的只读全局面板。E 的数据来自 Agent 的全局任务安全投影，按 Session 分组，不按 Project 分组。当前选中的 Session 可以有轻量标记，但 C 切换不改变 E 的数据范围。

每个任务只显示简短任务名称和当前状态。不显示工具来源、Turn、参数摘要、执行输出、详细错误或任务详情。没有可见任务的 Session 不显示空分组。

可见状态包括等待执行、运行中、等待资源、取消中、失败和已保存待处理结果。成功完成且结果已交付的任务不再显示。失败与待处理结果保持可见，直到 Agent 侧任务状态发生后续变化。

E 不提供取消、终止进程、停止全部、恢复、处理结果、右键菜单或详情展开。用户只能进入对应 Session，在 D 区通过聊天与助手操作任务。

## Widgetry 复用

ListView 负责任务行，Collapsible 可用于 Session 分组折叠，ScrollArea 负责滚动。任务行使用稳定 Task ID，不使用行索引关联业务身份。

## 工程边界

- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不拥有 Agent 业务状态。
- UI 使用 BSN、Scene 和 SceneList 构造与组合；Props 只初始化，运行状态由持久 Component / Resource 管理。
- 复用 `bevy_widgetry` 公开 API，不修改 Widgetry 上游、不访问其私有状态。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。GUI 行为使用真实应用和 BRP 验证；原生窗口行为在实际 Windows 窗口中观察。
## 完成条件与验收

1. 至少两个 Session 同时有任务时，E 同时显示两组任务，所属 Session 正确。
2. E 展示所有 Session 的可见任务，不只显示当前 C 选中的 Session。
3. C 切换后 E 的任务范围不变；当前 Session 标记随 C 变化。
4. 分组按 Session 归属，不按 Project；同一 Project 下两个 Session 分成两组。
5. 没有可见任务的 Session 不显示空分组。
6. 每行只显示任务名称和状态，不显示来源 Turn、工具来源、参数、输出或详细错误。
7. 任务名称来自 Agent 安全投影，不从原始命令或参数推测。
8. 等待执行、运行中、等待资源、取消中、失败、已保存待处理状态分别正确显示。
9. 成功且已交付任务从 E 移除；成功但未交付任务仍可见。
10. 任务状态更新后，E 在没有键鼠输入时也能刷新。
11. E 不存在取消、终止、停止全部、恢复、处理结果、详情展开或右键菜单。
12. 点击任务行不触发任何业务操作。
13. 任务历史仍可通过对应 Session 的存档恢复，不因从 E 移除而删除。
14. 使用 BRP 验收分组、滚动、状态更新和只读行为；截图确认只显示约定字段。
