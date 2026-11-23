# 持久化 Project 容器与 Session 归属

## 目标

让 Project 成为 Agent 侧可恢复的 Session 组织容器，而不是只存在于 C 区树控件中的临时分组。

## Project 身份与目录

Project 使用稳定 ID，记录项目名称、已存在的本地目录路径和所属 Session ID。创建 Project 必须选择一个已经存在的文件夹；名称默认使用文件夹名称，允许自定义。Project 元数据存放在 Agentelier 应用数据目录，不写入用户选择的项目文件夹。

Project 是 Session 的组织容器。每个 Session 最多属于一个 Project，也可以不属于任何 Project。重命名 Project 不改变 Project ID、Session ID 或工作目录；重命名 Session 不改变 Project 身份。

首批业务操作包括创建、重命名、删除 Project，将 Session 移入 Project、从 Project 移出，以及删除 Session 时清理对应归属记录。

## 删除 Project 的语义

删除 Project 时，所属 Session 自动转换为单独聊天：

- Project 组织记录被删除。
- 原 Session 历史、附件和执行记录保留。
- 原 Session 从 Project 分组移出，进入单独聊天列表。
- 用户选择的外部文件夹保留，不做递归删除。
- Session 原有工作目录快照不变。

## Agent 持久化边界

优先扩展 `agent/session_store` 的单写入者与 Archive Catalog 能力，保存 Project Catalog 和 Session 归属。Project 元数据可以作为独立的应用数据文件存放，但读写必须归 Agent 侧单一存储所有者，不放在 UI ECS。`agent/conversation` 接受 Project 归属和切换命令，但 Session 本身仍由 Conversation 管理。

## 工程边界

- Agent 业务事实归 `crates/agent`；UI 不维护第二份 Session、任务或资源目录权威状态。
- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。通过 Cargo 检查、实际应用运行、日志、存储记录和必要的调试观察验证。
- 只有涉及可观察 GUI 行为时才执行 BRP；BRP 不等价的 OS-level 行为使用真实 Windows 窗口验证。
## 完成条件与验收

1. 创建 Project 必须选择已经存在的目录；取消目录选择不创建 Project。
2. Project 名称默认取文件夹名称，用户可以自定义；改名不改变目录路径。
3. Project ID 稳定，不因重命名、排序或展开/收起变化。
4. Project 名称、目录和 Session 归属在应用重启后恢复。
5. Project 目录不存在、被移动或无权限时仍保留 Project 记录，并显示实际目录问题。
6. Session 移入 Project 后出现在该 Project 下；移出后进入单独聊天列表。
7. 一个 Session 不能同时属于两个 Project；重复归属请求被拒绝或幂等处理，不生成重复节点。
8. 删除 Project 后，全部所属 Session 自动转为单独聊天，Session 历史、附件和工作目录保留。
9. 删除 Project 不删除、不移动、不重命名用户选择的外部文件夹。
10. 删除 Session 后，其 Project 归属同步清理；其他 Session 和 Project 不受影响。
11. Project Catalog 保存失败时，UI 不提前显示成功；重启后恢复最后一次成功提交。
12. 并发创建、重命名和移动请求由 Agent 侧单一写入者串行处理，不出现部分写入或旧快照覆盖。
13. Project 元数据不写入用户工作目录，不引入项目 Skill 或 Project Context 扫描。
14. 路径展示不出现 `\\?\` 前缀，文件操作仍使用原始 PathBuf。
15. 通过真实应用创建、移动、删除 Project 和重启恢复验收，不通过直接修改 UI ECS 代替持久化验证。
