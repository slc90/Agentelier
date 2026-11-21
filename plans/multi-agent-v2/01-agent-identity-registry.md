# 建立 Agent 身份与 Registry

## 目标与交付边界

建立稳定 Agent 身份及 Session 级注册表，为后续树形关系和独立 Runtime 提供身份基础。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不能在此阶段实现完整 Spawn、Mailbox 或 UI Agent 浏览器。

## 设计与行为

Agent ID、Session ID、Thread/Conversation 身份映射；Root 注册；Agent 元数据；路径/昵称预留机制；注册/释放语义。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

Root 每个 Session 唯一；Agent ID 不跨 Session；预留可回滚；已提交身份不可被复用；Runtime 句柄不进入 Registry 持久字段。

## 验收场景与通过标准

以 gpt-6.2-sol 创建一个普通 Root 会话并检查 Root 登记；通过实际运行时诊断确认同一 Session 只有一个 Root；尝试重复注册、跨 Session ID 冲突和重复身份时得到明确拒绝；创建临时预留后模拟准备失败，确认名额/路径预留释放且无残留；检查关闭重开后的身份恢复由持久化增量负责，当前仅确认内存注册边界。

## 验证方式

编译相关 Crate；运行应用创建会话；查看日志/调试状态确认 Root 注册与重复拒绝；不新增测试代码。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 身份字段与所有权检查

| 对象 | 创建时确定 | 运行时可变 | 恢复要求 |
|---|---|---|---|
| Agent ID | 是 | 否 | 原值恢复 |
| Session ID | 是 | 否 | 必须匹配打开的 Session |
| Parent Agent ID | 是 | 否 | 父节点必须存在且属于同一 Session |
| Agent Path | 是 | 仅允许受控重建/迁移 | 与父子图核对 |
| Role | 是 | 仅由显式配置操作改变 | 使用保存的 Role 版本 |
| Runtime Instance ID | 每次启动生成 | 每次恢复更新 | 不从存档复活 |

Root 的身份由会话创建流程建立，不通过普通 Spawn 创建。Registry 对 Root 注册必须幂等：会话恢复时再次登记同一 Root 不生成第二个 Root；若同一 Session 出现两个不同 Root ID，应作为数据冲突处理。

## 额外验收边界

- 同一 Session 连续注册 Root 两次，第二次返回原身份或明确的已存在结果，不创建新条目。
- 不同 Session 可以拥有相同的相对 Agent Path，但不能共享 Agent ID 或 Conversation 历史。
- 预留路径后在持久化前取消，确认路径和总量预留均释放；提交后取消则保留身份并进入明确失败/停止状态。
- Agent 昵称重复时生成不冲突的显示名称，但工具解析仍以 Agent ID/Path 为准。
- Runtime 重建时 Agent ID 不变、Runtime Instance ID 改变；旧实例迟到事件不能更新新实例状态。
