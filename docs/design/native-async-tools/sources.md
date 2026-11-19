# 研究依据与采用范围

## 官方协议

| 资料 | 本方案采用的内容 |
| --- | --- |
| [Async tool calling](https://developers.openai.com/api/docs/guides/async-tool-calling) | 异步声明、原调用结果关联、应用管理工作、自定义同步等待工具；官方 task_handle 示例提供一种应用选择 |
| [Responses create](https://developers.openai.com/api/reference/resources/responses/methods/create) | developer `additional_tools` 输入项、Function Tool 字段、`call_id` 与输出项身份 |
| [SIWC preview limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations) | HTTP 使用 `store:false`、`stream:true` 和完整历史，省略 `previous_response_id`，支持 `additional_tools` |
| [Conversation state](https://developers.openai.com/api/docs/guides/conversation-state) | 手动管理有序历史，保留原始输出和推理续接材料 |
| [Compaction](https://developers.openai.com/api/docs/guides/compaction) | Inline 压缩、不透明项与工作窗口；本方案另行约束 pending 调用的安全位置 |
| [Using GPT-6.2](https://developers.openai.com/api/docs/guides/latest-model) | GPT-6.2 的 Responses 工具路径及历史前缀约束；不据此切换本次选定模型 |

2026-11-19 使用现有 Sign in with ChatGPT 与 `gpt-6.2-sol` 做了单变量对照。四组首轮均返回 HTTP 200，并回显相同的异步声明。

| 声明形态 | SSE 完整项和最终调用项 | 延迟结果 |
| --- | --- | --- |
| 顶层平铺 | 均为 `async:true` | 结果缺席时答出独立问题，随后按原 `call_id` 复述返回标记 |
| 固定 namespace | 均省略 `async` | 同步结果续接成功 |
| namespace 叶子指定 `allowed_callers:["direct"]` | 均省略 `async` | 同步结果续接成功 |
| developer `additional_tools` 输入首项 | 均为 `async:true` | 结果缺席时答出独立问题，随后按原 `call_id` 复述返回标记 |

`additional_tools` 对照的首个完成响应 ID 是 `resp_01ac26fe30f995cb016afed914295c87d1b064966fb3e2b018`。正式验收仍需覆盖应用的会话冻结、发送、存储和执行边界。

## 当前实现

以下链接指向本仓库内的核查入口，实施时仍以实际源码为准。

- [根规则](../../../AGENTS.md)、[项目级约束](../../../rules/project-context.md)、[开发流程](../../../rules/development.md)、[架构边界](../../../rules/architecture.md)、[文档规范](../../../rules/documentation.md)、[GUI 验证](../../../rules/gui-debugging.md)。
- [总体架构](../architecture.md)、[会话](../modules/conversation.md)、[模型请求](../modules/model-request.md)、[会话存储](../modules/session-store.md)、[长期记忆](../mechanisms/long-term-memory.md)。
- [协议历史](../../../crates/agent/llm_model/src/protocol.rs)、[流解析](../../../crates/agent/llm_model/src/stream.rs)、[请求准备](../../../crates/agent/llm_model/src/generation.rs)。
- [ActiveTurn](../../../crates/agent/conversation/src/active.rs)、[会话所有者](../../../crates/agent/conversation/src/conversation.rs)、[恢复](../../../crates/agent/conversation/src/restore.rs)、[压缩检查点](../../../crates/agent/conversation/src/compaction.rs)。
- [Runner](../../../crates/agent/turn_runner/src/runner.rs)、[Runtime](../../../crates/agent/runtime/src/runtime.rs)、[前台任务回收](../../../crates/agent/runtime/src/runtime/model_task.rs)。
- [工具注册](../../../crates/agent/tools/src/catalog.rs)、[进程会话](../../../crates/agent/tools/src/sessions.rs)、[MCP 调用](../../../crates/agent/mcp/src/call.rs)。
- [存储读取与记忆来源](../../../crates/agent/session_store/src/reader.rs)、[记忆提取](../../../crates/agent/memory/src/extraction.rs)。
- [聊天事件接收](../../../crates/ui/chat_ui/src/conversation.rs)、[工具时间线](../../../crates/ui/chat_ui/src/tool_timeline.rs)、[活动输入](../../../crates/ui/chat_ui/src/active_input.rs)。

旧设计中“生成期间不能发送”以及“Turn 完成代表全部工具资源结束”等表述，不能直接当成目标行为。前者需以当前 Steer 代码核对，后者正是本次要改变的边界。

## Codex 参考

只研究 `codex-rs/` 和 `docs/` 范围。以下路径采用源代码链接便于核查，不复制一套 Codex 架构。

| 入口 | 可借鉴的机制 |
| --- | --- |
| [turn_input.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/session/turn_input.rs) | 先 Steer，只有空闲才新建 Turn；接纳次序与单活动槽 |
| [tasks/mod.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/tasks/mod.rs) | 结束时输入接纳竞态、前台任务与会话资源的不同控制边界 |
| [input_queue.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/session/input_queue.rs) | 队列事实和唤醒分离、先订阅再检查 |
| [input_flow.rs](https://github.com/openai/codex/blob/main/codex-rs/tui/src/chatwidget/input_flow.rs) | Submitted 与显式 Queue 的交互区别 |
| [parallel.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/parallel.rs) | 共享/独占准入、可取消等待、真实执行与 completion future 分开 |
| [stream_events_utils.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/stream_events_utils.rs) | 完整调用项处早派发；本次保留更晚的完整响应持久提交边界 |
| [turn.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/session/turn.rs) | 当前代码在下一次采样前收齐工具结果，不能直接作为本方案跨 Turn 结果交付实现 |
| [process_manager.rs](https://github.com/openai/codex/blob/main/codex-rs/core/src/unified_exec/process_manager.rs) | 会话级进程身份、单项/全部终止与真实回收 |
| [responses_api.rs](https://github.com/openai/codex/blob/main/codex-rs/tools/src/responses_api.rs)、[models.rs](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/models.rs) | 本次受检类型未保留原生 async 字段；普通并发不能被当成原生异步协议 |

## 结构与界面参考

- [bevy_widgetry](https://github.com/slc90/bevy_widgetry)：使用现有 BSN/Scene 组合、公开控件状态更新和行生命周期方式。任务运行不交给 Widgetry 辅助任务体系。
- [bevy_brp](https://github.com/slc90/bevy_brp)：通过真实应用的 Pointer、Keyboard、截图、ECS 查询和日志完成 GUI 验收；请求接纳和最终 UI 行为分别核查。
- 当前依赖与工具能力以 [Cargo.toml](../../../Cargo.toml)、[Cargo.lock](../../../Cargo.lock) 及所锁定修订为准。参考库 main 的新增接口不是已安装能力，不为本次文档或异步功能顺便升级依赖。

这些仓库均作为只读参考。设计和实施方案只要求修改本应用范围内必要的模块与文档。
