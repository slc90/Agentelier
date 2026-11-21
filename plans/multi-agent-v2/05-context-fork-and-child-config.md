# 实现 Context Fork 与子 Agent 配置继承

## 目标与交付边界

定义 Full History Fork 与 Fresh Context，并按调用时有效配置形成子 Agent 配置快照。

本增量的交付是一个可以单独运行和观察的功能边界，包含实现、必要的状态/协议衔接、相关文档同步和下列验收。不负责创建 Runtime 和持久化 AgentCreated 事务。

## 设计与行为

历史边界、历史 Fork、Fresh Context、模型/推理继承、工作目录、执行权限、工具/技能/项目指令和审批策略继承、Role 叠加、配置快照。

### 与现有架构的关系

该能力归入 Agentelier 已有的 `agent/runtime`、`agent/conversation`、`agent/turn_runner`、`agent/session_store`、`agent/tools`、`agent/llm_model`、`agent/prompts` 和 Bridge 边界中实际承担该职责的模块。不得为了照搬 Codex 目录而建立平行 Runtime、平行 Conversation 或第二套会话存储。跨领域依赖遵守 `rules/architecture.md`：允许 `ui → agent`、`ui → bridge`、`bridge → agent`，禁止反向依赖。

### 必须保持的不变量

子 Agent 权限不得高于父级；使用当前调用步骤设置而非陈旧会话初始值；Fork 后历史独立；模型覆盖必须经过目录验证。

## 验收场景与通过标准

用 gpt-6.2-sol 的 Root 创建 Full History 与 Fresh Context 两种准备样例；Full History 子 Agent 能引用 Fork 边界之前的关键约束，Fresh Context 不应看到未传递的旧对话；Fork 之后父 Agent 新消息不自动出现在子 Agent 历史；检查模型、推理等级、工作目录、权限、Skills/项目指令来源与 Role 叠加；尝试让子 Agent Role 获得父级没有的工具/权限，必须拒绝；显式指定不支持 Multi-Agent 的模型或不支持的 reasoning effort，Spawn 配置准备失败且不创建身份。

## 验证方式

检查请求材料、配置快照和运行日志；验证当前 Agentelier 的 Prompt/Project Context/Skills 快照契约；无自动化测试代码。

项目禁止新增或维护自动化测试代码、Benchmark 和 Example。验收应使用真实应用、模型交互、日志、运行状态、存档核对和必要的 BRP，不把这些场景写成测试程序。Root 模型固定为 `gpt-6.2-sol`；需要子 Agent 时，只选择当前目录中已确认属于 GPT-6 及以上且非 Astra 级别的模型。实际 slug 和推理等级必须记录。

## 失败处理

所有失败都应保留 Agent ID、Session ID、Agent Path、原始 Turn/call ID、操作阶段和实际错误类别。若结果未知或持久化回执不确定，应阻断可能重复的副作用，先读取权威状态；不得通过重新 Spawn、重复消息或重放工具调用来掩盖不确定性。

## 配置快照需要覆盖的来源

子 Agent 配置快照至少应能解释以下值的来源：模型 slug、推理等级、工作目录、执行身份、权限/沙箱、审批策略、工具目录、MCP/Skills 可用性、项目指令快照、Prompt Snapshot、Role 版本和历史 Fork 边界。不要把完整父配置对象无差别复制后再期待所有字段都适用于子 Agent；应明确哪些字段继承、哪些字段重新解析、哪些字段禁止继承。

## 额外验收边界

- 父 Agent 在创建前刚刚改变模型/推理设置，子 Agent 使用调用步骤的有效设置，不使用旧 SessionSettings。
- 父 Agent 在 Spawn 后修改目录或项目指令，已创建子 Agent 的工作目录和指令快照不发生漂移。
- Full History Fork 的历史截止点准确；Fork 之后新增的父历史不进入子历史。
- Fresh Context 不包含父历史正文、旧工具调用或旧 reasoning，但保留明确指定的会话级项目上下文。
- 子 Agent 无权读取父级没有权限的路径、网络或工具；角色层不能扩权。
- 配置恢复时逐项核对快照哈希/来源，缺失关键安全配置时恢复失败，不回退到更宽松默认值。
