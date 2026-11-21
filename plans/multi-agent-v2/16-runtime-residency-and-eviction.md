# 实现 Runtime 驻留与安全卸载

## 目标与交付边界

在驻留容量限制下卸载空闲子 Agent Runtime，同时保留身份、历史、配置和 Mailbox。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不处理完整冷恢复流程；只提供可被下一阶段恢复的卸载产物。

## 设计与行为

驻留许可、候选选择、独占 gate、卸载前检查、Mail 转移、配置快照、关闭与驻留表更新。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

不能卸载 Root/运行中/正在收尾/有未确认提交 Agent；先停止接纳再迁移 Mail；精确实例校验防止误删替代 Runtime；卸载失败不释放容量。

## 验收场景与通过标准

设置小驻留上限，创建多个子 Agent 并使部分进入 Waiting；确认仅合格空闲 Agent 被卸载；Running Agent、持有存储提交的 Agent 和正在关闭的 Agent 不被选中；卸载期间并发到达 QueueOnly 消息，确认消息进入持久 Mailbox 且顺序保留；模拟 Shutdown 失败，确认 Registry 仍保留身份并标记错误、容量未错误释放；目标 Runtime 被替换时旧卸载操作不得移除新实例。

## 验证方式

观察驻留计数、状态事件、Mailbox 和 Join 回执；不使用性能基准。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 卸载候选与门闩

候选选择应只考虑空闲、可重建且不持有关键资源的子 Agent。卸载通过独占 residency gate 与运行操作竞争；无法取得 gate 时返回 Busy 并稍后重试，不强制杀死正在执行的 Agent。卸载过程要核对 Registry 中的 Runtime Instance ID，避免旧卸载任务清理刚刚恢复的新实例。

## 额外验收边界

- Root 永不成为自动卸载候选，即使 Root 当前没有生成内容。
- Agent 正在等待消息时可以成为候选，但其 Mailbox 必须在卸载后仍可接收并持久保存消息。
- Agent 刚完成工具调用但 Store 结果回执尚未确认时，不得卸载。
- 卸载开始后新 Follow-up 必须被排队或等待卸载完成，不得同时启动第二个 Runtime。
- 卸载完成后 Registry 显示 Unloaded，业务状态保留；失败后旧 Runtime 是否仍驻留以实际 Shutdown 结果为准。
- 驻留容量回收过程中并发创建多个 Agent，不得超出容量上限或双重占用同一 slot。
