# 文档导航

`docs/` 记录架构、模块设计与跨模块机制。具体实现以当前源码和工程配置为准；待实施的目标设计须明确标注状态，对应执行方案保存在 `plans/`，不得提前写成当前实现事实。

- [总体架构](design/architecture.md)：Workspace、程序入口、宿主与内部 Crate 的职责及依赖。
- [数据目录与启动](design/mechanisms/application-data.md)：配置、目录选择、启动检查及关闭边界。
- [Splash 启动演出与主窗口交接](design/modules/splash.md)：Splash 窗口会话、帧确认及数据目录初始化时序。
- [认证运行与凭据管理](design/modules/authentication.md)：账号持久化、认证工作者和凭据生命周期。
- [账号界面投影](design/modules/account-ui.md)：账号状态与界面职责边界。
- [模型请求](design/modules/model-request.md)：模型目录、请求协议、流式解析与取消。
- [会话与协议历史](design/modules/conversation.md)：固定快照、任务执行、历史提交、存档恢复与进程收尾。
- [会话存储](design/modules/session-store.md)：会话日志、执行事件、图片附件、提交回执及单写入者关闭边界。
- [提示词版本与快照](design/modules/prompts.md)：提示词版本、组合和固定指令的数据边界。
- [项目上下文](design/modules/project-context.md)：项目规则发现、有界采集、可恢复失败与来源核对。
- [Skills](design/skills/README.md)：来源发现、任务目录快照、隐式/显式调用与内置创建器的边界。
- [Markdown 展示](design/modules/markdown.md)：正文解析、实体投影与阅读状态。
- [Direct 工具调用](design/tool_call.md)：工具协议、执行循环、运行中补充与队列。
- [Computer Use](design/computer_use/computer-use.md)：Windows 窗口管理、截图、键鼠输入和 Rhai 执行。
- [Agent 自主 ECS 访问与动态 UI 构造](design/mechanisms/agent-ecs-access.md)：Rhai 到 Bevy 主线程的进程内请求、反射读写、事件派发和 BSN Scene 动态实例。
- [UI Automation](design/computer_use/ui-automation.md)：Computer Use 的控件语义操作设计，涵盖元素定位、Control Pattern、事件和缓存。
- [已知问题](known_issues.md)：已确认但尚未解决的上游限制及影响。

`design/modules/` 与 `design/mechanisms/` 分别保存模块与跨模块机制设计。按任务需要查阅相关内容，不另建 Feature 文档、功能目录或逐项功能使用说明。阶段目标设计应明确区分已实现的起点与待实施部分。

## 待实施的目标设计

- [原生异步工具与跨 Turn 结果交付](design/native-async-tools/README.md)：以现有 Direct 工具和会话结构为起点，说明跨 Turn 多任务、历史交付、取消、恢复与验收边界；当前实现状态仍以现有模块文档和源码为准。对应[执行方案](../plans/native-async-tools/00-overview.md)。
