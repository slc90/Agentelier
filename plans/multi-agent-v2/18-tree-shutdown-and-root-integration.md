# 完成 Agent Tree 关闭与 Root 对话集成

## 目标与交付边界

将 Root 的 Stop、会话关闭/删除和应用关闭与整棵 Agent Tree 生命周期接合，并完成不暴露内部协作细节的聊天体验。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不建设 Agent Tree 用户控制台、Trace Viewer、Telemetry 或 Analytics。

## 设计与行为

Session 级关闭门控、后代中断顺序、Runtime 回收、Mailbox 关闭、Store 排空、Bridge 事件投影、Root UI 最终状态。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

停止新 Spawn/副作用先于取消后代；等待真实资源回收和 Store join；UI 投影非权威；用户只与 Root 交互。

## 验收场景与通过标准

Root 正常委派多个子 Agent 后完成任务，用户只看到 Root 的最终答复与必要总体进度；Root Stop 时新 Spawn 被拒绝、活动子 Agent 收到停止并逐个真实回收，Mailbox 不再接纳新消息；关闭/删除 Session 时若存在活动资源按项目现有互斥规则拒绝或先协调停止，不能遗留孤儿；应用退出时 Runner、工具进程、Agent Runtime、Mailbox 和 Store 按所有权顺序关闭；模拟 Store 提交失败或子 Agent Shutdown 失败，UI 显示真实失败且不谎报全部完成；BRP 验证 Root 对话、Stop、状态收敛和无多余内部协作气泡。

## 验证方式

Root 固定 gpt-6.2-sol，子 Agent 仅用 GPT-6+ 非 Astra；真实 GUI 通过 BRP 操作与截图/状态检查；运行 Cargo 检查与 Clippy；不新增测试代码。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## Session 关闭顺序

关闭是 Session 级协调操作：先拒绝新 Spawn 和新副作用，再通知所有活动 Agent 停止，等待每个 Runner 与工具资源真实回收，处理待交付结果与 Mailbox，关闭 Agent Control，最后让 Session Store 排空已接纳提交并 join。顺序必须服从实际资源所有权，不能只按对象创建顺序猜测。

## 额外验收边界

- Root 正常完成用户请求但子 Agent 仍在工作时，产品策略必须明确是等待、保留后台还是在最终答复后继续；验收按该既定策略检查，不能让 Root final 隐式销毁子 Agent。
- 用户 Stop 当前 Root Turn 与停止整个 Agent Tree 是两个不同动作；前者按既定任务语义处理子 Agent，后者由 Session 关闭/全部停止路径完成。
- Session Reset、删除和应用退出时，活动子 Agent、待回收工具、未确认 Store 提交分别成为阻塞来源；不能只检查 Root Runner 是否空闲。
- Bridge 事件丢失时，UI 可通过重新读取 SessionView 收敛；UI 不能因某个事件先到就把业务状态标记为完成。
- BRP 验证普通用户视角只看到 Root 对话和最终结果，不出现子 Agent 内部工具调用气泡；必要的总体进度信息不暴露敏感路径或提示词。
- 完整关闭后确认没有活动 Runner、工具进程、Mailbox watcher、驻留许可和未 join 的 Store Worker。
