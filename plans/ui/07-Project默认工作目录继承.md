# 让 Project 目录成为新 Session 的默认工作目录

## 目标

在 Project 中创建新 Session 时，默认把 Project 目录写入 Session 的 `working_directory`，同时保留 Session 创建后的独立修改能力。

## 目录关系

Project 保存默认目录；Session 创建时将当时的 Project 目录复制到自己的 `SessionSettings.working_directory`。创建完成后，两者成为独立值。

- 修改 Project 目录只影响以后创建的 Session。
- 修改已有 Session 的工作目录只影响该 Session。
- 将已有 Session 移入 Project 或移出 Project，不自动改变它原来的工作目录。
- 单独聊天不继承当前 Project 的目录，仍由 Session 创建准备阶段指定工作目录。
- 重启后从 Session 存档恢复工作目录，不重新读取 Project 当前目录覆盖快照。

`project_context` 继续根据 Session 自己的 `working_directory` 发现 `AGENTS.md` 等材料。Project 容器不成为第二套上下文发现机制。

## Agent 影响

`crates/agent/conversation` 在创建 Session 时接收最终工作目录，并保存到 `SessionSettings` / `InitialSession`。`crates/agent/session_store` 保存 Project 目录与 Session 工作目录快照。`crates/agent/project_context` 仍然只读取当前 Session 的工作目录。

## 工程边界

- Agent 业务事实归 `crates/agent`；UI 不维护第二份 Session、任务或资源目录权威状态。
- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。通过 Cargo 检查、实际应用运行、日志、存储记录和必要的调试观察验证。
- 只有涉及可观察 GUI 行为时才执行 BRP；BRP 不等价的 OS-level 行为使用真实 Windows 窗口验证。
## 完成条件与验收

1. 在 Project 中创建 Session 时，初始工作目录等于 Project 当前绑定目录。
2. 创建前用户修改该 Session 工作目录后，最终创建结果使用用户指定目录。
3. Session 创建后，修改 Project 目录不会改变已有 Session 工作目录。
4. 修改 Project 目录后，新建 Session 使用新目录。
5. 将 Session 移入另一个 Project，不自动修改其工作目录。
6. 将 Session 从 Project 移出为单独聊天，不自动修改其工作目录。
7. 重启后打开 Session，恢复存档中的工作目录，不从 Project 当前目录重新推导。
8. Project 目录被移动或删除时，已有 Session 仍保留原路径，工具执行按真实路径错误处理，不自动跳到新目录。
9. 单独聊天不继承上次选中的 Project 目录。
10. 同一 Project 下两个 Session 可分别使用不同工作目录，`project_context` 按各自路径发现规则。
11. 工作目录改变不修改 Session 模型、推理强度、账号绑定、提示词快照或 ToolCatalog。
12. 路径显示不出现 `\\?\` 前缀，文件操作使用原始 PathBuf。
