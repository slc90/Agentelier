# 文档导航

`docs/` 记录设计文档。探索与尚未实施的方案保存在 `plans/`，不得提前作为既有行为写入本文档区。

- `features/`：面向使用者的功能、对外行为与限制。只有新增实质功能时补充。
- `design/architecture.md`：Workspace、应用与内部 Crate 之间的当前关系。
- `design/modules/`：具体模块的核心设计。
- `design/mechanisms/`：跨模块机制的核心设计。
- `known_issues.md`：确认但尚未解决的上游限制及影响。

根据任务需要查阅相关文档；具体实现以当前代码为准。程序入口与通用宿主的边界见[应用宿主与程序入口](features/application.md)。

已实现的目录选择、配置保存与启动占用流程见[数据目录与启动](design/mechanisms/application-data.md)，文件账号恢复与后台生命周期见[认证运行与凭据管理](design/modules/authentication.md)。浏览器登录、按需会话续期与退出登录已接入；[模型连接验证](features/model-request.md)已接入，通信与取消关系见[单次模型请求](design/modules/model-request.md)，历史实施顺序见[登录增量导航](../plans/登录/00-overview.md)。

模型准备区已接入[聊天与流式回复](features/chat.md)，会话及执行关系见[内存聊天与流式投影](design/architecture.md#内存聊天与流式投影)。

准备区的[提示词管理](features/prompts.md)支持来源版本、组合预设和固定会话指令；数据边界见[提示词版本与快照](design/modules/prompts.md)。

助手正文采用[原生 Markdown 展示](design/modules/markdown.md)，支持范围和阅读行为见[聊天功能](features/chat.md)。

聊天已接入 Direct 工具调用、连续决策、同批串行、运行中补充和会话待办队列。使用方式见[聊天功能](features/chat.md)，完整设计关系与边界见[Direct 工具调用](tool_call.md)。
