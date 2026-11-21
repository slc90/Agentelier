# 实现 list_agents

## 目标与交付边界

为 Agent 提供当前 Session 内 Agent Tree 的可读状态视图。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不建设用户可见完整 Agent 控制台。

## 设计与行为

稳定排序、路径前缀过滤、父子关系、Role、状态、驻留摘要和安全字段过滤。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

列表是只读快照；不返回凭据/绝对敏感路径；列表状态不能作为写操作授权依据。

## 验收场景与通过标准

Root 通过 list_agents 查看多层树，确认每个 Agent 的 ID/Path/父级/Role/业务状态正确且排序稳定；按路径前缀过滤只返回对应子树；列表在创建、等待、完成、中断、卸载和恢复后反映新状态；跨 Session Agent 不可见；确认响应不包含 access token、credential、私有绝对路径或内部句柄；空树和分页/上限边界返回明确结果。

## 验证方式

真实模型工具调用并与 Registry/Store 状态比对；如 UI 展示状态则使用 BRP 检查。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 投影字段与稳定性

列表用于模型规划，不是完整诊断转储。每项应包含稳定 Agent ID、Path、父级/子级关系、Role、业务状态、驻留摘要和必要的任务名称。字段需要有界，列表排序稳定，内容不包含访问令牌、环境变量、内部 JoinHandle、私有文件内容或无关绝对路径。

## 额外验收边界

- 创建/完成/中断/卸载/恢复各发生一次后，下一次 List 都能反映对应状态，不依赖 UI 缓存。
- 路径前缀过滤只按完整路径段匹配，不把 `root/a` 错误匹配为 `root/ab`。
- 分页/结果上限下，重复请求相同游标不会漏项或重复项；若不分页则明确返回截断标记和继续查询方式。
- Agent 刚从 Registry 移除但 Store 仍有历史时，List 不应返回可操作的幽灵 Agent。
- 列表读取不能改变 Agent 状态、唤醒 Agent 或加载 Unloaded Runtime。
