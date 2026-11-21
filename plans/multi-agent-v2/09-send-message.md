# 实现 send_message

## 目标与交付边界

向指定 Agent 发送 QueueOnly 消息，不因目标空闲而自动启动新 Turn。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不实现 Follow-up 的唤醒语义。

## 设计与行为

参数校验、目标解析、消息编码、Mailbox 接纳、投递回执、发送方/接收方关系。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

空消息拒绝；目标必须同 Session；工具成功代表消息已接纳；send_message 不触发空闲 Agent 执行。

## 验收场景与通过标准

以 Root gpt-6.2-sol 向正在运行的子 Agent 发送消息，确认当前执行可在安全边界读取；向 Waiting/Unloaded Agent 发送消息，确认工具返回成功且目标不启动模型请求；随后显式 Follow-up 时才开始处理；向不存在、跨 Session、已关闭目标发送消息应失败；发送空白消息、超长消息和非法内容时得到明确参数错误；确认消息的原始发送者、call ID 和接收者持久可查。

## 验证方式

真实模型工具调用；对比发送前后目标请求数/状态；日志与 Store 记录交叉核对。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 工具参数与返回契约

`target` 必须是同一 Session 中可解析的 Agent ID 或完整 Agent Path；`message` 必须非空。工具只负责投递，不承诺目标已经阅读或执行。成功结果表达 Mailbox 已接纳，可返回消息 ID 和目标状态；不能用“已完成任务”这类误导性文案。

## 额外验收边界

- 向目标 Running Agent 发送消息时，目标当前执行不会被强制打断；消息按安全边界可见。
- 向 Waiting Agent 发送消息时，目标仍保持 Waiting，不产生新模型请求。
- 向 Unloaded Agent 发送消息时，不必为了 QueueOnly 投递完整加载模型 Runtime，但持久 Mailbox 必须可接纳。
- 向调用者自身发送消息是否允许应由工具契约明确；若不允许，应返回自发送拒绝，不得陷入自我唤醒循环。
- 接收方关闭/Session 正在关闭/队列已满时，工具返回失败且消息没有被误报为已接纳。
