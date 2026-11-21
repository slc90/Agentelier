# 建立 Agent Tree 状态与目标解析

## 目标与交付边界

在稳定 Agent 身份上建立父子树、路径索引和状态语义，并提供 Session 内安全目标解析。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不实现模型工具入口和消息投递。

## 设计与行为

父子关系、唯一 Agent Path、深度字段、状态枚举、路径/ID 目标解析、子树查询。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

树必须无环、无孤儿、父节点同 Session；Root Path 唯一；状态和驻留状态分离；路径不是权限凭据。

## 验收场景与通过标准

通过调试入口建立 Root→A→A1 与 Root→B 的树，检查父子/路径/深度双向一致；尝试环、重复路径、缺失父节点、跨 Session 父节点，均应拒绝；通过 Agent ID 和 Path 定位同一 Agent；未知路径、模糊昵称、已关闭 Agent 和跨 Session 目标必须返回不同且可诊断的错误；确认 Agent Waiting 与 Runtime Unloaded 不被合并成同一个业务状态。

## 验证方式

直接运行应用/调试入口检查 Registry 状态；日志核对拒绝原因；无 GUI 时不强制 BRP。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 状态模型与路径规则

Agent 业务状态和 Runtime 驻留状态必须分别存储。业务状态描述任务是否正在执行、等待、完成或中断；驻留状态描述执行环境是否加载。不能用 `Evicted` 覆盖任务的 `Waiting`，也不能因为 Runtime 不在内存就认为 Agent 已完成。

路径生成遵循父路径加唯一子段的规则。子段可以采用受控 slug 或 Registry 分配的稳定段；显示昵称不参与路径唯一性。路径解析应先做规范化，再做 Session 所属检查，避免大小写、分隔符或前缀近似造成越权命中。

## 额外验收边界

- 验证 `root/a`、`root/ab` 等前缀相似路径不会被错误视为同一个节点。
- 验证路径包含空段、`.`、`..`、重复分隔符或超长输入时被拒绝或规范化到唯一确定结果。
- 完成态 Agent 可以保留在树中并接受 Follow-up；删除/关闭态 Agent 不可被普通消息目标解析命中。
- Agent 被卸载时业务状态保持不变，只有驻留状态变化；恢复时不得把 `Completed` 自动改成 `Running`。
- 查询子树时返回父节点与后代关系一致，不返回其他 Session 的节点。
