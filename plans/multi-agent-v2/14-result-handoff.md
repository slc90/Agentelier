# 完成子 Agent 结果交付与 Root 汇总

## 目标与交付边界

将子 Agent 完成结果作为可持久化业务事实交给父 Agent，并支持 Root 基于结果继续协调和汇总。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不做完整 Rollout Trace 和用户可见 Agent 活动面板。

## 设计与行为

AgentTaskCompleted、父级通知、结果关联、错误摘要、重复交付去重、后续请求上下文材料。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

结果先持久化后通知父级；每个结果关联原任务和 Agent；重复通知不重复追加历史；Root 自己决定最终答复。

## 验收场景与通过标准

让两个子 Agent 分别返回成功结果和失败摘要，Root 能在后续请求中读到准确来源并汇总；让子 Agent 完成后模拟父级暂时不可运行，恢复父级后仍能收到一次结果；重复完成通知不重复交付；检查结果不会混入用户原始消息或冒充 Root Assistant 发言；子 Agent 的错误不能被转换成成功；Root 可以基于结果安排新的 Follow-up。

## 验证方式

检查 Conversation 历史项、Store 序号、父子任务来源与 Root 模型请求材料；真实模型语义核验与结构事实分开记录。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 结果交付的提交顺序

子 Agent 的最终结果先由其 Conversation 形成不可变完成事实，再通过 Session Store 提交，之后通知父 Agent。父 Agent 读取结果时应包含原始任务、完成状态、来源 Agent 和结果正文/错误摘要。若父 Agent 正在运行，结果作为内部活动输入，在安全边界可见；若父 Agent 卸载，则保留待交付状态。

## 额外验收边界

- 子 Agent Completed 但结果 Store 提交失败时，不能通知父 Agent 结果已完成；会话应阻断后续依赖该结果的执行并显示存储错误。
- Store 提交成功但父 Agent 通知失败时，恢复后能够重新投递通知而不重复写结果。
- 父 Agent 已消费结果后再次收到相同通知，不能在模型历史中重复追加同一结果。
- 子 Agent Failed/Interrupted 的结果必须保留终态和错误摘要，不得生成空成功结果。
- Root 汇总时能区分不同子 Agent 的结论和冲突；不能把一个子 Agent 的结论误标为 Root 已验证事实。
