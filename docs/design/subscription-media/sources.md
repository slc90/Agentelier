# 设计依据与参考边界

## 当前工程与约束

- [Agentelier 总体架构](https://github.com/slc90/Agentelier/blob/main/docs/design/architecture.md)：单进程 Bevy/Tokio、Agent/Bridge/UI 职责边界。
- [认证运行](https://github.com/slc90/Agentelier/blob/main/docs/design/modules/authentication.md)、[模型请求](https://github.com/slc90/Agentelier/blob/main/docs/design/modules/model-request.md)：Codex 凭据所有权、模型目录与请求冻结、原生异步工具声明。
- [会话与存储](https://github.com/slc90/Agentelier/blob/main/docs/design/modules/conversation.md)、[Session Store](https://github.com/slc90/Agentelier/blob/main/docs/design/modules/session-store.md)、[图片结果](https://github.com/slc90/Agentelier/blob/main/docs/design/local-development-tools/04-image-results.md)：调用身份、会话恢复、附件约束。
- [原生异步工具设计](https://github.com/slc90/Agentelier/blob/main/docs/design/native-async-tools/README.md)、[当前工具定义](https://github.com/slc90/Agentelier/blob/main/crates/agent/tools/src/catalog.rs)、[当前请求构造](https://github.com/slc90/Agentelier/blob/main/crates/agent/llm_model/src/generation.rs)：当前源码保留历史 `additional_tools` 的读取契约，并通过 Codex 接入原生异步。新增媒体工具应复用当前执行链。
- [项目级规则](https://github.com/slc90/Agentelier/blob/main/rules/project-context.md)、[架构规则](https://github.com/slc90/Agentelier/blob/main/rules/architecture.md)、[文档规则](https://github.com/slc90/Agentelier/blob/main/rules/documentation.md)：Windows x64、禁止持久化测试代码、设计文档的存放位置与身份保护。

## 官方 Codex（仅以 codex-rs 为参考）

- [Codex 模型服务路由](https://github.com/openai/codex/blob/main/codex-rs/model-provider-info/src/lib.rs)：ChatGPT 认证模式的服务地址。
- [Codex 凭据格式](https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/storage.rs) 与 [TokenData](https://github.com/openai/codex/blob/main/codex-rs/login/src/token_data.rs)：`auth.json` 结构、工作区与来源。
- [图片生成工具](https://github.com/openai/codex/blob/main/codex-rs/ext/image-generation/src/tool.rs)、[Images Endpoint](https://github.com/openai/codex/blob/main/codex-rs/codex-api/src/endpoint/images.rs)：`gpt-image-2`、请求/返回与工具结果。
- [音频输入协议](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/models.rs)、[本地音频准备](https://github.com/openai/codex/blob/main/codex-rs/user-input/src/local_media.rs)、[转写客户端](https://github.com/openai/codex/blob/main/codex-rs/backend-client/src/client.rs)：音频类型与转写输入形式。

## 官方公开接口与授权边界

- [Sign in with ChatGPT Preview Limitations](https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations)：第三方凭据不支持图片生成、音频输入/转写、原生 Computer Use 等工具。
- [GPT Image 生成接口](https://developers.openai.com/api/docs/guides/image-generation) 与 [Responses 图片工具](https://developers.openai.com/api/docs/guides/tools-image-generation)：普通 API 的行为与模型选项；不能直接等同于 Codex OAuth 服务。
- [音频文件转写](https://developers.openai.com/api/docs/guides/speech-to-text) 与 [Audio in Chat Completions](https://developers.openai.com/api/docs/guides/audio-chat-completions)：文字转写和直接音频理解的机制不同。
- [Async Tool Calling](https://developers.openai.com/api/docs/guides/async-tool-calling)：工具结果与原调用身份的关联。

## 其他参考仓库（只借鉴适用机制）

- [bevy_widgetry FileDialog 核心设计](https://github.com/slc90/bevy_widgetry/blob/main/docs/design/modules/file_dialog.md)：已有受管文件选择 Session、模式、过滤与确认，不重造文件选择组件。
- [bevy_brp](https://github.com/slc90/bevy_brp/blob/main/README.md)：真实 Bevy GUI 的运行时截图、交互和状态验证工具；不成为 Agent 功能实现依赖。

## 已有兼容性实验的事实

先前的独立实验使用用户指定的官方 Codex CLI `auth.json`，只读运行，确认普通 Responses、`gpt-image-2` 生成有效 PNG，以及 `gpt-6-astra` 在同一 WebSocket 后续回合的异步工具继续与原 `call_id` 结果交付。本设计只将这些**已经实际观察的组合**作为能力依据；音频服务没有同等实验结论。
