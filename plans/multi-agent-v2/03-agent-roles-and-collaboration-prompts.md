# 加入 Agent Role 与协作指令

## 目标与交付边界

建立职责 Role 和 Root/Sub-Agent 协作指令快照，让 Root 知道何时委派、子 Agent 知道任务边界。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不在本增量实现模型覆盖和 Spawn 配置叠加。

## 设计与行为

默认 Role、自定义 Role 解析；角色指令；Root 协作模式 Explicit Request Only / Proactive / Custom；子 Agent 行为指引；Prompt snapshot。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

Role 不等于 Skill；Role 只能收窄权限；未知或损坏 Role 不静默回退；指令快照与版本/哈希关联。

## 验收场景与通过标准

以 gpt-6.2-sol 进行三组真实对话：Explicit Request Only 下未明确请求时不 Spawn；Proactive 下复杂任务能提出合理委派并由工具实际创建；Custom 按给定策略改变委派行为；为研究/实现/审查 Role 分别创建 Agent，确认角色指令进入对应请求；配置一个试图启用父 Agent 不具备能力的 Role，必须拒绝或剔除越权项并记录；修改提示词库后，已创建 Agent 继续使用旧快照，新 Agent 使用新快照。

## 验证方式

检查请求检查视图/安全日志中的指令来源与哈希；模型结果按行为判定，不要求逐字一致；若 UI 有新增展示，按 GUI 规则 BRP 验证。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 指令组合与覆盖优先级

Root 协作指令、子 Agent 通用约束、Role 指令、Spawn 任务文本和用户项目指令是不同来源。请求组装应沿 Agentelier 已有 Prompt Snapshot 规则保留来源和顺序，不把 Role 文本拼接进用户消息。子 Agent 的职责指令不能覆盖系统安全边界或父级权限。

Explicit Request Only、Proactive、Custom 是 Root 的委派策略，不是子 Agent 的执行状态。Custom 的来源应固定为会话创建时的配置快照；配置缺失时不得猜测用户偏好。

## 额外验收边界

- Role 仅修改模型偏好时，确认工具权限、工作目录和执行身份仍继承父级。
- Role 请求禁用某项父级已有能力时，子 Agent 请求中不再声明该能力。
- Role 请求启用父级没有的能力时，拒绝该项并返回明确配置错误，不能因为配置层合并顺序而扩大权限。
- Prompt 库更新后，正在运行的 Agent 继续使用创建时快照；新 Agent 才取得新版本。
- Custom 策略为空、格式损坏或超出允许长度时，按明确配置错误处理，不静默切换 Proactive。
