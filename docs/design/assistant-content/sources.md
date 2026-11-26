# 源码与技术参考

本组设计从当前 Workspace、现有设计文档和已讨论的交互目标出发。以下入口用于核对真正的职责和已存在的实现，代码与工程配置优先于历史实施方案。外部技术只作为选型、能力参照或公开接口依据，未将其写成已经接入的功能。

## 当前实现依据

| 主题 | 相关源码与设计 |
| --- | --- |
| 主聊天区现有筛选、行复用、等待与 Memory 投影 | [`conversation.rs`](../../../crates/ui/chat_ui/src/conversation.rs)（`ChatMessage`、`ChatTurn`、`RowContent`、`final_reply`、`reconcile_list`）；[Conversation](../modules/conversation.md)；[UI](../ui/README.md) |
| 公开消息和工具结果投影 | [`active.rs`](../../../crates/agent/conversation/src/active.rs)（`StreamEvent`、`ToolView` 更新）；[Conversation Bridge](../../../crates/bridge/conversation/src/conversation.rs)（`MessageView`、`ToolImageView`、`TurnView`） |
| 图片原图保存、预览与导出 | [`archive.rs`](../../../crates/agent/conversation/src/archive.rs)（`ToolReturned`、`GeneratedImage`）；[`attachments.rs`](../../../crates/agent/session_store/src/attachments.rs)；[`image_attachment.rs`](../../../crates/agent/runtime/src/runtime/image_attachment.rs)；[图片生成设计](../subscription-media/02-image-generation.md) |
| 音频导入、原件及附件卡片 | [`audio.rs`（Conversation）](../../../crates/agent/conversation/src/audio.rs)；[`audio.rs`（Store）](../../../crates/agent/session_store/src/audio.rs)；[`audio_attachment.rs`](../../../crates/ui/chat_ui/src/audio_attachment.rs)；[音频文件理解](../subscription-media/03-audio-file-understanding.md) |
| Markdown 解析、全文增量和原文 | [`parse.rs`](../../../crates/ui/markdown/src/parse.rs)；[`view.rs`](../../../crates/ui/markdown/src/view.rs)；[Markdown](../modules/markdown.md) |
| Rhai 和 ECS 请求契约 | [`script.rs`](../../../crates/agent/tools/src/script.rs)；[`ecs_access.rs`](../../../crates/agent/tools/src/ecs_access.rs)；[`script/ecs.rs`](../../../crates/agent/tools/src/script/ecs.rs)；[ECS Access 设计](../mechanisms/agent-ecs-access.md) |
| Scene Factory 与模板 | [`agent_host/src/ecs_access/scenes.rs`](../../../crates/ui/agent_host/src/ecs_access/scenes.rs)；[`application/src/agent_scenes.rs`](../../../crates/ui/application/src/agent_scenes.rs)；[AgentHost](../../../crates/ui/agent_host/src/host.rs) |
| Session 单写入者、回执和恢复 | [`store.rs`](../../../crates/agent/session_store/src/store.rs)；[`reader.rs`](../../../crates/agent/session_store/src/reader.rs)；[`journal.rs`](../../../crates/agent/session_store/src/journal.rs)；[Session Store](../modules/session-store.md) |
| Skills | [Skills 设计](../skills/README.md)；[`skill-creator`](../../../crates/agent/skills/assets/skill-creator/SKILL.md) |

[Workspace 配置](../../../Cargo.toml) 与 [Cargo.lock](../../../Cargo.lock) 中，Bevy 锁定为 0.20.1，Rhai 为 1.27.0（`sync`、`no_module`），pulldown-cmark 为 0.13.6，Symphonia 为 0.6.1（MP3、WAV、PCM、ID3）。Bevy Feature 当前未包括音频；Widgetry 已锁定依赖包含可选 Canvas、Slider、Number Input、Waveform 等 Feature，具体使用时按公开 API 和实际配置接入。

## 技术参考

- [Bevy AudioPlayer](https://docs.rs/bevy/0.20.1/bevy/audio/struct.AudioPlayer.html)、[AudioSinkPlayback](https://docs.rs/bevy/0.20.1/bevy/audio/trait.AudioSinkPlayback.html) 与 [Bevy 0.20.1 Cargo Features](https://docs.rs/crate/bevy/0.20.1/source/docs/cargo_features.md) 提供原生播放、暂停、停止、位置与格式 Feature 依据。MP3、FLAC、WAV 的实际播放在实施时运行验证。
- [bevy_widgetry](https://github.com/slc90/bevy_widgetry)：Canvas、Waveform、Slider、Number Input 等既有原生组件的公开实现。Waveform 的当前输入语义偏向信号采集，不能当成直接发声的播放器。
- [pulldown-cmark 0.13.6](https://docs.rs/crate/pulldown-cmark/0.13.6)、[Options](https://docs.rs/pulldown-cmark/latest/pulldown_cmark/struct.Options.html)：数学扩展与事件解析。
- [LaTeX-Rust](https://github.com/jscarr64/LaTeX-Rust)：首选评估的纯 Rust 排版引擎，项目说明提供 SVG/PNG 及数学布局，MSRV、宏与专业覆盖以实际验证为准。
- [MathJax TeX 扩展目录](https://docs.mathjax.org/en/v4.0/input/tex/extensions/)、[KaTeX](https://katex.org/)：符号与宏环境的覆盖参考，不是本次引入的 Web UI 技术。
- [Desmos API](https://www.desmos.com/api/v1.2/docs/index.html)、[JSXGraph](https://jsxgraph.org/docs/)：函数和几何可视化的语义参考。
- [OpenAI Generative UI 示例](https://github.com/openai/openai-structured-outputs-samples/tree/main/generative-ui)：结构化组件描述与应用侧渲染的参考，不采用其网页渲染架构。
- [Vega-Lite 参数绑定](https://vega.github.io/vega-lite/docs/bind.html)：参数、选择与数据联动。
- [Rhai Book](https://rhai.rs/book/)、[Bevy Dynamic ECS 示例](https://github.com/bevyengine/bevy/blob/main/examples/ecs/dynamic.rs)、[bevy_mod_scripting](https://github.com/makspll/bevy_mod_scripting)：嵌入式脚本与运行时动态机制的参考，继续复用当前 Rhai/ECS 执行框架。
- [Agent Skills 规范](https://agentskills.io/specification)：保持标准 Skill 目录，不定义专有控件包格式。

## 项目约束

适用规则包括 [AGENTS.md](../../../AGENTS.md)、[架构](../../../rules/architecture.md)、[项目环境](../../../rules/project-context.md)、[UI Composition](../../../rules/ui-composition.md)、[开发流程](../../../rules/development.md)、[GUI 验证](../../../rules/gui-debugging.md)、[文档](../../../rules/documentation.md) 和 [任务范围](../../../rules/task-scope.md)。以 Windows x64 / DX12 / PowerShell 7、Workspace 统一外部依赖、原生 BSN、Type-Driven Development 为前提；实际 GUI 变化需 BRP，禁止新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 程序，也不得为调试改变登录凭据或数据目录。正常依赖 API 不满足目标时，遵守既有禁止修改上游源码的规则，并记录真正影响使用的已知问题。
