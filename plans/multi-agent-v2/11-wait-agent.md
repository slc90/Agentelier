# 实现 wait_agent

## 目标与交付边界

让 Agent 使用事件驱动方式等待协作活动或超时，而非轮询其他 Agent 状态。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不把 Wait 扩展成跨 Agent 阻塞式 Join；不改变目标 Agent 的执行状态。

## 设计与行为

timeout 参数、默认/最小/最大时间、Mailbox 活动订阅、Steer 中断、超时返回、等待生命周期。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

订阅必须先于检查待处理活动，避免丢唤醒；超时范围受配置约束；取消等待不取消目标 Agent。

## 验收场景与通过标准

用 gpt-6.2-sol 让 Root Wait 一个子 Agent：消息先到、等待中消息到、超时、Root 收到 Steer、等待任务被取消五种场景分别验证；消息在订阅建立前后竞态不能丢唤醒；超出最大 timeout 返回明确错误，低于最小值按约定钳制并说明；Wait 被取消后子 Agent 继续执行；连续等待不会产生忙轮询日志或 CPU 空转。

## 验证方式

真实工具调用和时间线日志；观察等待时长与唤醒原因；不以固定模型回复文本作为唯一标准。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## Wait 的竞态处理

Wait 必须先订阅活动通知，再检查当前 Mailbox/活动状态，或使用等价的无丢失通知协议。否则消息可能恰好在“检查为空”和“开始等待”之间到达，导致 Agent 永久睡眠。Wait 监听的是调用者自身的协作活动，不把任意其他 Agent 状态变化都当作可消费消息。

## 额外验收边界

- 等待开始前已有待处理消息时，Wait 应立即返回活动可用，而不是等下一次新消息。
- 消息在订阅建立与进入 await 的竞态窗口到达时，Wait 必须被唤醒。
- 超时与消息同时发生时，返回结果必须按实际先完成的活动/超时规则确定，不重复消费消息。
- Root Steer 可以打断当前 Wait，并让 Root 的输入接纳规则继续工作；Wait 取消不能中断被等待的子 Agent。
- 应用关闭时 Wait 被取消并释放订阅，不留下持续 watch receiver。
