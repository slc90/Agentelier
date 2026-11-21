# 参考机制与现有结构

这里把参考分成现有起点、采用的上游机制和本阶段的取舍。来源中的同名能力不等于需要整套移植；会话跨账号共享、简化侧栏和只做自动 Remote V2 是本阶段自己的边界。

## 现有结构与约束

| 材料 | 支撑的事实与本阶段衔接 |
| --- | --- |
| [工程入口与文档职责](https://github.com/slc90/Agentelier/blob/main/AGENTS.md) | 按任务读取规则；当前设计与方案区分；修改架构后同步相关文档。本次设计材料单独放在 `docs/design/session-context/`，不覆盖已有模块说明。 |
| [项目环境与验证约束](https://github.com/slc90/Agentelier/blob/main/rules/project-context.md) | Windows x64、DX12、PowerShell 7；禁止测试、Benchmark、Example；允许真实应用与 BRP 验证；保护登录状态。 |
| [开发流程](https://github.com/slc90/Agentelier/blob/main/rules/development.md) | 实际修改前维护临时进度记录；按小功能迭代；编译和静态检查不能代替运行结果。 |
| [架构规则](https://github.com/slc90/Agentelier/blob/main/rules/architecture.md) | Agent、Bridge、UI 依赖方向，Workspace 依赖管理，禁止修改版上游，`app` 薄入口。 |
| [UI 组合规则](https://github.com/slc90/Agentelier/blob/main/rules/ui-composition.md) | BSN、Scene、SceneList；Props 一次初始化，运行状态由 ECS 保存。 |
| [GUI 验证规则](https://github.com/slc90/Agentelier/blob/main/rules/gui-debugging.md) | 实际输入、截图及状态核对；释放 Pointer；不能直接改 ECS 冒充交互。 |
| [文档规则](https://github.com/slc90/Agentelier/blob/main/rules/documentation.md) | 模块与机制设计，不新增 Feature 文档层；代码注释和文档语言边界。 |
| [当前总体架构](https://github.com/slc90/Agentelier/blob/main/docs/design/architecture.md) | 唯一 AgentHost/Connection、Runtime 任务所有权、Conversation 与 Runner 的关系。 |
| [会话模块](https://github.com/slc90/Agentelier/blob/main/docs/design/modules/conversation.md) | 单个内存 Session、历史封闭、Steer、Queue、工具清理、图片预算。 |
| [会话身份与账号同步源码](https://github.com/slc90/Agentelier/blob/main/crates/agent/conversation/src/conversation.rs) | 当前 Session 使用进程内计数，设置含 `account_subject`，账号变化清除内存会话。本阶段改为持久身份和任务级执行身份。 |
| [协议历史源码](https://github.com/slc90/Agentelier/blob/main/crates/agent/llm_model/src/protocol.rs) | 原始助手与 reasoning 项、call/result 配对、类型化图片载荷；UI 摘要不是请求权威。 |
| [模型目录源码](https://github.com/slc90/Agentelier/blob/main/crates/agent/llm_model/src/model.rs) | 当前目录只贯通身份、推理和图片能力，窗口元数据需要新增。 |
| [生成请求源码](https://github.com/slc90/Agentelier/blob/main/crates/agent/llm_model/src/generation.rs) | 当前请求地址、`store=false`、完整历史、加密续接、账号校验和 32 MiB 序列化限制。 |
| [聊天状态与发送门控源码](https://github.com/slc90/Agentelier/blob/main/crates/ui/chat_ui/src/conversation.rs) | 当前单会话投影、草稿与展开状态、UI 中的账号相等判断。 |
| [数据目录机制](https://github.com/slc90/Agentelier/blob/main/docs/design/mechanisms/application-data.md) | 用户选择的数据根、`config/config.ron`、本次运行固定路径、目录占用与关闭释放。 |
| [提示词快照](https://github.com/slc90/Agentelier/blob/main/docs/design/modules/prompts.md) | `prompts.json` 与 `InstructionSnapshot`；会话指令不依赖提示词库继续存在。 |
| [聊天与 Markdown 设计](https://github.com/slc90/Agentelier/blob/main/docs/design/chat-foundation/05-ui-markdown.md) | 右侧页面的输入、阅读跟随、公开控件 API 和原生 Markdown；其中早期单轮限制以当前工具及队列设计为准。 |
| [现有依赖配置](https://github.com/slc90/Agentelier/blob/main/Cargo.toml)与[已知问题](https://github.com/slc90/Agentelier/blob/main/docs/known_issues.md) | 复用现有依赖与平台约束；BRP 特殊键及切页文字现象按已有记录处理。 |

`sessions/<id>/` 的具体文件组织、独立 Session Store、最简列表以及本次运行内的 UI 状态保存，是围绕这些现有职责确定的适配设计，不是对当前源码已经具备这些能力的描述。

## Codex 的存档与恢复

[RolloutRecorder](https://github.com/openai/codex/blob/main/codex-rs/rollout/src/recorder.rs)采用 JSONL 记录、串行写入命令、Flush 确认和可等待的工作者关闭。这里采用这些职责关系，写入业务记录和附件；不复制它的多种存储后端、分支或其他客户端接入。

[历史规范化](https://github.com/openai/codex/blob/main/codex-rs/core/src/context_manager/normalize.rs)为缺少结果的函数调用生成与 `call_id` 对应的 `aborted` 请求项。源码明确区分恢复时的请求规范化和真实持久工具结果。本阶段保持这种区别，展示未知执行事实，不自动执行旧参数。

[模型上下文扫描](https://github.com/openai/codex/blob/main/codex-rs/rollout/src/model_context.rs)以同时具备替换历史和窗口号的最近压缩记录作为重建边界。这支持“完整存档继续保存、请求只重建当前窗口”的分离，而不是把摘要当作全部磁盘历史。

本阶段不采用上游账号归属过滤，不将会话创建者作为恢复权限，不建设 Codex 文件格式的导入器，也不复制它的测试代码。

## Codex 的窗口与 Remote V2

[模型参数](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/openai_models.rs)定义上下文窗口解析、有效比例及自动阈值规则；[窗口判断](https://github.com/openai/codex/blob/main/codex-rs/core/src/session/context_window.rs)区分有效窗口硬边界和自动压缩阈值。本阶段采用总工作上下文口径，不增加多种计量模式设置页。

[ContextManager](https://github.com/openai/codex/blob/main/codex-rs/core/src/context_manager/history.rs)区分历史代次、服务端使用量锚点和之后新增项的估算。恢复后的工作窗口变化需要重新建立计量依据，不能把累计消费或旧内存索引当成当前占用。

[Remote V2 请求准备](https://github.com/openai/codex/blob/main/codex-rs/core/src/compact_remote_v2_attempt.rs)将 `compaction_trigger` 加入 Responses 输入，完成后从检查点输入中移除这个请求控制项。[Remote V2 执行与保留规则](https://github.com/openai/codex/blob/main/codex-rs/core/src/compact_remote_v2.rs)检查合法完成及唯一压缩项，构造包含保留消息和压缩记录的新历史。

本阶段只适配这条远端协议及对应的自动请求边界，不引入本地摘要、手动触发、投机式后台压缩、多 Agent、Hook 或音频保留体系。服务请求地址和认证方式由现有模型层继续管理；不能因公开 API 中存在另一个压缩入口，就将它当作相同协议替换进来。

## Widgetry 与 BRP

[Widgetry ListView](https://github.com/slc90/bevy_widgetry/blob/main/docs/features/list_view.md)区分数据源、稳定业务条目、投影顺序、选择与物理行复用。侧栏直接利用这些公开能力，不把行 Entity 或位置当作 Session 身份。列表库支持搜索与拖动，不意味着本次 UI 要开放这些功能。

[Widgetry 文档入口](https://github.com/slc90/bevy_widgetry/blob/main/docs/README.md)说明 BSN 组合与运行状态的边界。侧栏、编辑、确认和滚动沿用应用已经使用的公开组件，不在应用内复制控件。

[BRP 的连接和输入契约](https://github.com/slc90/bevy_brp/blob/main/README.md)提供实际应用发现、输入、截图和状态观察。普通 Pointer 是独立 Custom Pointer，不等于操作系统鼠标；工具返回成功也不等于 UI 行为成功。只采用运行验证能力，不将参考库中的测试宿主、测试脚本或 Example 移植进本项目。
