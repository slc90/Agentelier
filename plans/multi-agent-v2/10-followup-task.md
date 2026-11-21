# 实现 followup_task

## 目标与交付边界

向已有 Agent 投递新任务并触发目标执行，支持对活动 Agent 的有序后续输入。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不负责 Wait 工具和 Agent 总并发限制策略。

## 设计与行为

TriggerTurn 投递模式、空闲目标启动、运行中目标后续输入、幂等唤醒、任务来源。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

消息接纳与唤醒请求关联；不对同一 Agent 启动并行 Turn；同一 Follow-up 不重复执行；已卸载目标可按需恢复。

## 验收场景与通过标准

向 Waiting Agent 发 Follow-up，确认其启动新 Turn 并处理消息；向 Running Agent 发 Follow-up，确认当前 Turn 安全边界后按既定输入策略处理，不能并发第二个 Turn；重复同一 followup ID 不重复执行；向 Unloaded Agent 发 Follow-up，确认恢复后再执行；验证任务来源仍指向原发送 Agent/Turn，不因目标恢复改变；Root 收到完成状态并可继续委派。

## 验证方式

检查目标 Agent Turn 序号、Mailbox 消费事实、实际模型请求和结果交付；异常情况下核对不会重复副作用。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 唤醒与输入接纳

Follow-up 的成功必须同时满足消息已接纳以及目标已获得可执行的唤醒安排。若目标当前正在执行，唤醒安排是下一安全边界的后续输入，不代表另开并行 Turn。若目标卸载，则先创建恢复请求；恢复失败时 Follow-up 保留可查询的失败/待处理状态，不能返回虚假的执行成功。

## 额外验收边界

- 空闲 Agent 收到 Follow-up 后只启动一个 Turn，输入中包含消息内容和发送来源。
- Running Agent 连续收到多条 Follow-up 时按已定义顺序处理，消息不合并丢失来源。
- 目标 Agent 的父级已被关闭时，不得继续启动失去所有者的子树。
- 重复 Follow-up ID 只触发一次执行；同 ID 不同消息内容应冲突。
- 目标恢复失败时，消息保持可见且状态明确；Root 可以查询失败并选择新的补救任务。
