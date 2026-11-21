# 实现 Agent 冷恢复与故障收敛

## 目标与交付边界

按原身份恢复已卸载 Agent 的历史、有效配置和未消费消息，并将崩溃中的不确定任务收敛为明确状态。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不重新执行上次未确认的工具副作用，不自动发模型请求。

## 设计与行为

恢复祖先链、恢复配置快照、历史读取、模型选择校验、Mailbox 重建、执行实例 ID 更新、未知结果占位。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

恢复 Agent ID/Path 不变；Runtime instance ID 更新；不复活 OS/Tokio 句柄；结果未知不自动重跑；恢复失败可诊断。

## 验收场景与通过标准

先正常卸载 Agent，再由 Root Follow-up 触发冷恢复，确认身份/路径/历史/Role/模型/推理/工作目录/工具能力和 Mailbox 恢复；恢复后的 instance ID 与卸载前不同；已消费消息不重放，待消费消息只消费一次；模拟执行开始已保存但结果缺失的崩溃窗口，恢复后标记结果未知/中断，不再次运行原工具；模型不再可用时恢复明确失败，不静默切换到 Astra；重复恢复请求不会产生两个 Runtime。

## 验证方式

重启应用后重新打开同一 Session，核对 Store 事件与真实恢复行为；保留原登录状态；检查不自动触发模型请求。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 恢复阶段和不确定状态

冷恢复应拆分为身份恢复、配置解析、历史读取、Mailbox 重建、Runtime 启动和任务状态收敛。每个阶段都要保留错误阶段和目标 Agent ID。恢复只还原已提交事实；未确认的执行结果不能通过重新执行工具来“补齐”。

## 额外验收边界

- 恢复链中的父 Agent 缺失、路径与存档不一致或配置快照损坏时，拒绝启动目标 Runtime，并保留可诊断错误。
- 模型目录暂时不包含原模型时，不静默换用更昂贵或更高权限模型；按配置恢复策略暂停并要求明确处理。
- 已完成的历史任务保持 Completed；Wait 状态可恢复为 Waiting；执行中断的任务进入明确中断/结果未知状态。
- 冷恢复过程中同一 Agent 收到多个 Follow-up，只能合并到一个待启动队列，不能创建多个 Runtime。
- 恢复启动失败后重试不得重复消费 Mailbox 或重复交付已完成结果。
- 关闭应用后重开同一 Session，Root 可列出 Unloaded 子 Agent，并按需恢复其中一个；不自动加载所有后代。
