# 持久化、恢复与现有模块集成

Agent Tree 的可恢复性建立在 Agentelier 现有 Session Store 上，不新增第二套会话数据库。Conversation 定义 Agent 身份、消息、任务与状态转换；Session Store 以单写入者方式提交事件；Runtime 持有真实执行资源；Bridge 将业务事件转换为 UI 可安全消费的投影。

## 持久化内容

需要持久化的内容包括：

- AgentCreated：Agent ID、Session ID、父 Agent、Agent Path、角色、Spawn 来源、Fork 边界和配置快照引用。
- AgentStateChanged：业务状态变化及原因。
- AgentMessageQueued / AgentMessageConsumed：消息 ID、发送者、接收者、投递模式、顺序和消费事实。
- AgentTaskAssigned / AgentTaskCompleted：任务来源、原始调用 ID、任务内容引用、结果或失败摘要。
- AgentInterrupted / AgentClosed：中断请求、真实执行终态和资源回收事实。
- AgentRuntimeEvicted / AgentRuntimeRestored：驻留状态变化及恢复实例身份。

事件名称可以与现有 `RecordEvent` 命名协调，但语义必须能区分请求已接纳、消息已投递、消息已消费、取消已请求和资源已真实回收。不能用一个笼统的 `AgentUpdated` 事件覆盖所有阶段。

## 提交与执行边界

Agent 创建应先原子提交 Agent 身份及父子边，再启动 Runtime。消息先由 Mailbox 接纳并持久化，再向发送方确认。任务开始前持久化启动事实；结果保存后再交付父 Agent。若提交结果不确定，阻断可能重复的副作用并重新读取权威日志，不盲目重试。

Session Store 继续沿用现有 JSONL 日志、单写入者、连续序号、Flush/sync 回执和摘要可重建原则。Agent 图和消息事实属于所属 Session，不另设全局跨会话存储。存储线程关闭时必须排空已接纳提交，并在实际 join 后再释放数据环境。

## 恢复规则

打开 Session 时先恢复 Root Conversation 与 Agent 图元数据；不自动启动所有子 Agent Runtime。用户继续任务或 Root 需要联系某个子 Agent 时，Agent Control 再按需恢复目标 Runtime。已消费消息不会再次消费；已交付结果不会重复交付；只持久化了消息投递但未消费的消息保留在 Mailbox。

如果执行开始记录存在而完成结果不存在，恢复应将其标记为上次执行中断或结果未知。工具副作用可能已经发生，禁止自动重新执行原工具调用。父 Agent 读取该状态后决定是否安排新的补救任务。

## 模块职责映射

| 现有模块 | Multi-Agent 承接职责 |
|---|---|
| `agent/runtime` | Agent Control、Runtime 创建/恢复/关闭、资源准入与驻留协调 |
| `agent/conversation` | Agent 身份与业务状态、Agent Tree 事实、Mailbox 业务状态、任务和结果关联 |
| `agent/turn_runner` | 每个 Agent 的独立模型—工具循环及终态交付 |
| `agent/session_store` | Agent 图、消息、任务与生命周期事件的持久化 |
| `agent/llm_model` | 模型目录能力元数据、模型请求与实际模型信息 |
| `agent/tools` | 六个协作工具的声明、参数验证和 Control 调用适配 |
| `agent/prompts` | Root/Sub-Agent 协作指令与角色提示词快照 |
| `bridge/conversation` | Agent 事件和状态的安全业务投影 |
| `bridge/connection` | 保持唯一连接、命令/事件传递和可靠唤醒 |
| `ui/agent_host` | 继续作为唯一宿主和事件分发入口，不持有 Agent 业务权威 |
| `ui/chat_ui` | 展示 Root 对话及必要任务摘要，不提供完整 Agent 操作控制台 |

这些是职责映射，不要求照搬 Codex 的 crate 划分，也不预先新建占位 crate。新增 Crate 必须符合 `rules/architecture.md` 的真实职责边界和依赖方向。

## 用户体验与可诊断性

用户看到 Root 的正常对话、最终答复以及必要的整体任务进度。子 Agent 的内部消息、工具调用、路径和调度细节不作为普通聊天气泡展示。开发者需要定位问题时，依靠结构化协作事件、Agent ID/Path、任务来源和明确状态错误；不构建完整 Rollout Trace 图或产品 Analytics。

## 防御性约束

- 不让 Agent、Tools 或 Runner 直接写 JSONL；统一经过 Conversation/Runtime 的业务提交路径。
- 不让 Bridge/UI 反向成为 Agent 状态权威。
- 不持久化 OS 句柄、Tokio 句柄、锁或 watch receiver。
- 不恢复旧进程，不重放未知结果的副作用。
- 不把用户可见的 Root 聊天历史和内部 Agent 历史混成同一条对话。
- 不因本功能修改账号凭据、OAuth 状态或用户数据目录占用规则。
