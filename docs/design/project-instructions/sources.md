# 依据与取舍

## local

### 当前模块事实

以下路径用于定位现有设计与源码，不作为要求重写这些模块的逐文件清单。

| 材料 | 本方案据此保留的事实 |
| --- | --- |
| `crates/agent/prompts/src/snapshot.rs` | 基础覆盖与有序补充形成固定快照；`messages()` 输出 developer 消息；默认空配置不自动生成基础文本 |
| `docs/design/modules/prompts.md`、`docs/design/modules/conversation.md` | 基础快照、会话目录、输入与重试身份的既有生命周期 |
| `crates/agent/conversation/src/active.rs` | ActiveTurn 独占活动协议历史，在请求处计量并准备当前输入 |
| `crates/agent/turn_runner/src/runner.rs` | 请求、响应存档、串行工具批次、Steer、取消和进程回收的顺序 |
| `crates/agent/llm_model/src/generation.rs` | store=false、完整输入重放、固定工具声明、32 MiB 序列化限制及同次认证恢复复用载荷 |
| `crates/agent/llm_model/src/protocol.rs` | 类型化协议项、工具调用与结果封闭，以及安全的 compaction 截取边界 |
| `crates/agent/conversation/src/context.rs`、`compaction.rs` | 用量锚点、窗口身份、检查点确认与安装边界 |
| `crates/agent/conversation/src/archive.rs`、`docs/design/modules/session-store.md` | 业务事件、权威日志、单写入者及恢复规则 |
| `docs/design/architecture.md`、`docs/design/modules/model-request.md` | UI／Bridge／Agent 依赖方向和当前 Inline 压缩路径 |

### 工程约束

`AGENTS.md` 规定按需读取 `rules/`、`docs/` 和相关方案。本次直接影响设计的规则为 `rules/project-context.md`、`rules/architecture.md`、`rules/development.md`、`rules/documentation.md`、`rules/ui-composition.md`、`rules/gui-debugging.md` 与 `rules/common-commands.md`。

据此保留 Windows x64／DX12／PowerShell 7、无 Bevy 的 Agent／Bridge 边界、BSN 构造与公开 Widgetry API、禁止测试代码和 Example、真实 BRP 验证、任务进度记录及登录状态保护。执行方案中的验证随具体功能交付，不另建测试工程。

## codex

| 原始来源 | 可以直接借鉴的机制 |
| --- | --- |
| [项目文件发现与来源组合](https://github.com/openai/codex/blob/main/codex-rs/core/src/agents_md.rs) | 从根到 cwd 的有序链、项目边界、来源记录和文件读取预算 |
| [指令管理器](https://github.com/openai/codex/blob/main/codex-rs/core/src/agents_md_manager.rs) | 应用中的指令快照，以及环境选择、信任状态和预算对项目缓存的影响 |
| [项目指令上下文状态](https://github.com/openai/codex/blob/main/codex-rs/core/src/context/world_state/agents_md.rs) | 内容不变不发新片段；变化发替换通知；清空发移除通知 |
| [User 指令的协议包装](https://github.com/openai/codex/blob/main/codex-rs/core/src/context/user_instructions.rs) | 项目指令使用 user 角色，与 Developer 消息区分 |
| [官方 AGENTS.md 说明](https://developers.openai.com/codex/guides/agents-md) | 项目路径上的层级发现与文件约定 |

本方案只使用 `AGENTS.md`，不带入全局来源、Thread Provider、override／fallback 文件名、企业信任管理和远端环境。项目链在每次逻辑请求前重新读取；这是本地应用的方案选择，不照搬 Codex 的缓存失效条件。超出预算明确失败，不截断规则。

## protocol

[Keep the prefix unchanged](https://developers.openai.com/api/docs/guides/preserved-reasoning)说明前缀变更会影响缓存和后续 reasoning 的保留。本方案据此保留已发送项目项，采用追加更新，而非覆盖旧前缀。

[Compaction](https://developers.openai.com/api/docs/guides/compaction)说明服务端 Inline compaction 及无状态输入数组续接方式。本方案继续使用现有服务端压缩，只增加新窗口中的项目原文恢复；不引入独立压缩请求、configuration_update 或自制摘要。

## ui

[Widgetry 文档入口](https://github.com/slc90/bevy_widgetry/blob/main/docs/README.md)与 [TextField 公共契约](https://github.com/slc90/bevy_widgetry/blob/main/docs/features/text_field.md)用于确认 BSN 构造、运行状态和只读文本的选择／复制能力。实现沿用工程当前依赖，不为了本功能升级或修改上游库。

[Bevy BRP 的使用契约](https://github.com/slc90/bevy_brp/blob/main/README.md)说明真实应用连接、Custom Pointer 和输入完成状态。验证采用应用现有 BRP 链路；参考库自己的测试、Example 和回归脚本不进入本项目。
