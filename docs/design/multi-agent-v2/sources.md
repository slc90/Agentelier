# 参考资料与依据

本设计以用户提供的 Agentelier 仓库、Codex 仓库和 Bevy 辅助仓库为研究范围。Codex 只研究 `codex-rs/` 与 `docs/`；其余仓库内容不作为本次参考。下列链接指向具体设计入口，实施时应结合当前默认分支实际内容核对。

## Agentelier 当前架构与规范

- [总体架构](https://github.com/slc90/Agentelier/blob/main/docs/design/architecture.md)
- [架构规则](https://github.com/slc90/Agentelier/blob/main/rules/architecture.md)
- [项目背景与验证约束](https://github.com/slc90/Agentelier/blob/main/rules/project-context.md)
- [GUI 调试与 BRP 验证](https://github.com/slc90/Agentelier/blob/main/rules/gui-debugging.md)
- [文档规则](https://github.com/slc90/Agentelier/blob/main/rules/documentation.md)
- [Conversation 设计](https://github.com/slc90/Agentelier/blob/main/docs/design/modules/conversation.md)
- [Session Store 设计](https://github.com/slc90/Agentelier/blob/main/docs/design/modules/session-store.md)
- [原生异步工具设计](https://github.com/slc90/Agentelier/tree/main/docs/design/native-async-tools)
- [原生异步工具执行方案](https://github.com/slc90/Agentelier/tree/main/plans/native-async-tools)
- [Agent 工具调用执行方案](https://github.com/slc90/Agentelier/tree/main/plans/Agent%20工具调用)
- [初步开发路线](https://github.com/slc90/Agentelier/blob/main/plans/Agentelier-初步开发路线.md)

## Codex Multi-Agent v2

- [Multi-Agent v2 工具入口](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents_v2.rs)
- [Spawn 工具](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs)
- [消息工具共用路径](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents_v2/message_tool.rs)
- [Wait 工具](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/multi_agents_v2/wait.rs)
- [Agent Control API](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/control/api.rs)
- [Agent Registry](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/registry.rs)
- [Mailbox](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/control/mailbox.rs)
- [Spawn 生命周期](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/control/spawn.rs)
- [子 Agent 配置继承](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/child_config.rs)
- [Agent Role](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/role.rs)
- [Runtime 执行准入](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/control/execution.rs)
- [Runtime 驻留管理](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/control/residency.rs)
- [Agent 数据类型](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/types.rs)
- [Agent 目标解析](https://github.com/openai/codex/blob/main/codex-rs/core/src/agent/agent_resolver.rs)
- [协作指令](https://github.com/openai/codex/blob/main/codex-rs/prompts/src/multi_agent_instructions.rs)

## 其他参考仓库

- [bevy_widgetry](https://github.com/slc90/bevy_widgetry)：仅在未来确实需要 GUI 控件或交互投影时参考其公开 API；Multi-Agent 核心业务不依赖 Widgetry。
- [bevy_brp](https://github.com/slc90/bevy_brp)：涉及 Chat UI 可观察变化时用于真实应用 BRP 自动化验证。遵循 Agentelier 的 `rules/gui-debugging.md`。

## 依据的使用方式

Codex 参考用于确认工具语义、消息投递模式、Agent 注册、Spawn 配置、驻留和恢复机制。Agentelier 的现有规范优先决定依赖方向、单写入者、持久化、运行时所有权、验证方式和 Windows x64 目标约束。其他仓库只在对应边界需要时使用，不因其存在而引入新的抽象。
