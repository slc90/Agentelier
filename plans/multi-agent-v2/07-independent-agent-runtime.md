# 让每个 Agent 拥有独立执行循环

## 目标与交付边界

让每个 Agent 使用独立 Conversation 与 Turn Runner，并由 Runtime 统一管理执行句柄。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不实现完整 Mailbox 语义和资源卸载。

## 设计与行为

AgentRuntime 实例、独立 Conversation、Runner、每 Agent 单活动 Turn、会话共享工具目录但调用上下文隔离、取消句柄和结果事件通道。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

一个 Agent 至多一个活动 Turn；不同 Agent 可并发；子 Agent 执行不占 Root 前台 Turn；不把异步工具任务冒充 Agent Runtime。

## 验收场景与通过标准

以 gpt-6.2-sol Root 同时创建两个子 Agent，子 Agent 选择符合条件的 GPT-6+ 非 Astra 模型；让两者执行有可观察时长的独立任务，确认执行区间重叠且历史/请求 ID/状态隔离；让单个子 Agent 同时接收两个启动请求，确认第二个被排队或拒绝而非并行占用同一 Conversation；Root 在子 Agent 运行期间继续正常接收 Steer/Queue；停止一个子 Agent，确认兄弟和 Root 不被误停；确认 Runner 的结果通过 Runtime 返回，不直接写 UI。

## 验证方式

检查模型请求 ID、Turn ID、Agent ID、运行状态和取消回执；运行 Cargo 检查；不编写测试代码。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## Runner 隔离与控制面

每个 Agent 的 Runner 使用独立的活动 Turn 状态和请求历史，但不建立独立的 Tokio Runtime 线程或第二个全局调度系统，除非现有 Runtime 的线程模型确有要求。Agent Control 是统一的控制面，持有 Agent 句柄并协调各 Runner。Conversation 仍是业务状态权威，Runner 不可直接修改另一个 Agent 的历史。

## 额外验收边界

- 两个子 Agent 同时调用工具时，工具执行上下文、原始 call ID、目录和取消令牌互不串用。
- 子 Agent 请求失败只影响该 Agent；Root 仍可查询它的失败状态并继续其他工作。
- Root 请求取消时，不应仅关闭 Root Runner 就遗留子 Agent 的活动任务。
- 同一 Agent 已有活动 Turn 时，Follow-up 只能排队或按既定输入边界接纳，不能产生第二个活动 Runner。
- 会话切换、Reset 和删除沿用 Agentelier 对活动任务/资源回收的互斥规则，不能因为子 Agent 是“后台”就绕过。
