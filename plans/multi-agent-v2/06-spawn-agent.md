# 实现可靠 Spawn

## 目标与交付边界

将 Spawn 配置、身份预留、持久登记和独立 Agent 启动串成一个可恢复创建操作。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。依赖后续独立 Runtime 的基础接口；本增量可先提供最小可启动 Runner 接口，不实现全部资源限制。

## 设计与行为

spawn_agent 参数/结果；预留/回滚；AgentCreated 持久事实；启动状态；幂等来源 call ID；创建失败归因。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

先提交身份和父子关系再启动；未提交不能执行；提交后启动失败要保留 Failed 身份；同一调用不能重复 Spawn。

## 验收场景与通过标准

以 gpt-6.2-sol Root 实际调用 spawn_agent 创建两个不同任务，确认返回不同稳定 Agent ID/Path；对同一 call ID 重放登记流程确认只产生一个 Agent；模拟路径预留失败、配置准备失败、持久提交失败和 Runtime 启动失败，分别确认无副作用或保留 Failed 身份；启动成功后子 Agent 首次请求使用指定任务和继承快照；Root 可查询创建结果。

## 验证方式

真实模型工具调用、Store 日志序号、Agent Registry 状态和执行事件交叉核对；创建 GUI 状态投影时按 BRP 验证。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## Spawn 的事务阶段

Spawn 应明确区分准备、资源预留、配置构建、身份提交、Runtime 启动和结果发布。各阶段的失败语义不同：身份提交之前可以回滚所有临时预留；身份提交之后不能删除已确认的创建事实，只能追加失败/关闭事实。工具响应只有在 Agent 身份和创建结果达到约定提交点后才能返回。

## 额外验收边界

- 两个并发 Spawn 竞争同一父 Agent 路径空间时，Registry 只能分配两个唯一子路径。
- 同一原始工具调用重复进入处理流程时，返回既有创建结果，不创建第二个子 Agent。
- 持久化提交成功但 Runtime 启动失败时，列表中存在该 Agent 且状态为 Failed，错误包含失败阶段。
- 创建失败不能遗留占用的并发许可、驻留许可或路径预留。
- 子 Agent 第一次请求的上下文包含任务说明、Fork 内容和 Role 指令，不包含其他子 Agent 的历史。
- Root 收到 Spawn 结果后可以立即使用返回的 Agent ID/Path 调用其他协作工具。
