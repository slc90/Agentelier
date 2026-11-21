# 扩展模型目录的 Multi-Agent 能力

## 目标与交付边界

让模型目录表达模型是否支持 Multi-Agent v2 及可用推理等级，供 Spawn 和模型覆盖校验。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不在本增量实现子 Agent 创建或验收策略 UI。

## 设计与行为

ModelInfo 能力字段；目录解析；模型能力未知/禁用/支持的区分；推理等级映射。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

能力必须来自目录数据或明确映射；不能仅凭 slug 字符串推断支持；未知状态 fail closed。

## 验收场景与通过标准

刷新真实模型目录并检查支持、禁用、未知三类模型；对目录缺失字段和异常类型执行拒绝/未知处理；确认现有普通模型选择与请求不受影响；对 gpt-6.2-sol 检查目录项实际存在和推理等级；记录子 Agent 可用候选中哪些属于 GPT-6+ 且非 Astra，不满足时停止模型验收而非猜测。

## 验证方式

用真实账号目录请求和模型选择界面核对；检查解析日志；不触发重新 OAuth。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 目录兼容与请求前校验

模型目录可能随账号、服务端和时间变化。Multi-Agent 能力字段应保留三态：明确支持 v2、明确不支持、目录未说明。后两者不能被当作支持。现有普通模型请求不依赖 Multi-Agent 字段，因此目录扩展应保持普通单 Agent 模型路径兼容。

模型 slug、显示名称、代际、Astra 级别、推理等级和 Multi-Agent 能力是不同属性。验收筛选子 Agent 模型时不能只检查字符串前缀；实际模型目录无法提供判断依据时，需人工核实模型目录项。

## 额外验收边界

- 目录刷新后某模型从支持变为不支持，新 Spawn 不得继续选择它；已有 Agent 的历史模型快照保留原值并在恢复时进行可用性检查。
- 模型支持 Multi-Agent 但不支持某个 reasoning effort 时，覆盖请求应失败并列出可用等级。
- 模型目录项重复、缺少 slug 或 capability 类型异常时，不得覆盖已确认有效目录。
- Root 固定 `gpt-6.2-sol` 的验收必须核对实际请求模型，而非仅核对 UI 选择框显示值。
- 子 Agent 候选排除 Astra 级别模型，即使它在目录中支持 Multi-Agent，也不能用于本轮验收。
