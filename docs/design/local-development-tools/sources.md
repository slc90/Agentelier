# 参考依据与设计取舍

这里列出对本阶段有直接作用的来源。源码说明“当前实际是什么”；讨论确定范围；设计文档负责写清楚本阶段选择如何接入。外部项目中的能力、默认值和工程规则不自动成为本项目要求。

<a id="current"></a>
## 当前应用：循环、历史与架构

| 来源 | 支持的事实或用途 |
| --- | --- |
| [工具目录](https://github.com/slc90/Agentelier/blob/main/crates/agent/tools/src/catalog.rs) | 当前目录登记计算和等待两个诊断工具，工具名称与执行实现配对。 |
| [工具执行](https://github.com/slc90/Agentelier/blob/main/crates/agent/tools/src/execution.rs) | 当前 15 秒统一期限、64 KiB 参数和结果限制、字符串结果、取消分支。 |
| [任务循环](https://github.com/slc90/Agentelier/blob/main/crates/agent/turn_runner/src/runner.rs) | 当前 32 次模型请求、64 次工具调用预算；串行批次、Steer 和停止边界。 |
| [会话设置与工具投影](https://github.com/slc90/Agentelier/blob/main/crates/agent/conversation/src/conversation.rs) | 会话设置快照、固定工具目录、当前展示记录的结构。 |
| [活动任务记录](https://github.com/slc90/Agentelier/blob/main/crates/agent/conversation/src/active.rs) | 当前 `tool_result` 与 `seal` 的结果保存路径。图片扩展需要把真实载荷与展示摘要分离。 |
| [协议历史](https://github.com/slc90/Agentelier/blob/main/crates/agent/llm_model/src/protocol.rs) | 当前 `FunctionResult` 为字符串，封闭步骤校验调用与结果的对应关系。 |
| [模型目录](https://github.com/slc90/Agentelier/blob/main/crates/agent/llm_model/src/model.rs) | 当前 `ModelInfo` 保存模型与推理设置，没有保存图片输入能力字段。 |
| [任务启动与回收](https://github.com/slc90/Agentelier/blob/main/crates/agent/runtime/src/runtime/model_task.rs) | Runtime 等待 Runner 真实结束后归还活动记录；取消期间继续排空事件。 |
| [总体架构](https://github.com/slc90/Agentelier/blob/main/docs/design/architecture.md) | UI、Agent、Bridge 三领域、current-thread Tokio、唯一 Connection 与宿主退出关系。 |

<a id="rules"></a>
## 工程约束

[AGENTS.md](https://github.com/slc90/Agentelier/blob/main/AGENTS.md)、[项目级约束](https://github.com/slc90/Agentelier/blob/main/rules/project-context.md)、[开发流程](https://github.com/slc90/Agentelier/blob/main/rules/development.md)、[架构规则](https://github.com/slc90/Agentelier/blob/main/rules/architecture.md)、[代码规则](https://github.com/slc90/Agentelier/blob/main/rules/code.md)、[日志规则](https://github.com/slc90/Agentelier/blob/main/rules/logging.md)共同约束本阶段：Windows x64 / PowerShell 7，保留现有账号状态，使用正常应用运行与静态检查验证，不在应用仓库新增测试、Benchmark、Example 或专用测试设施。

[Workspace 配置](https://github.com/slc90/Agentelier/blob/main/Cargo.toml)提供当前依赖和 lint 事实：`unsafe_code = "forbid"`；Tokio 当前未启用 `process`；新依赖及 feature 由 Workspace 统一管理。实现使用安全依赖封装，不在应用代码中绕开 unsafe 禁令。

[已知问题](https://github.com/slc90/Agentelier/blob/main/docs/known_issues.md)记录 BRP 特殊按键逻辑键值问题，以及多个启用 BRP 的实例争用固定 Render 端口的限制。更换 Main 端口不能解决后者。

<a id="codex"></a>
## Codex：参考行为，不复制整套宿主

| 来源 | 本阶段参考什么 |
| --- | --- |
| [Shell 工具定义](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/shell_spec.rs) | `exec_command`、`write_stdin`、工作目录、等待窗口、持续进程 ID 与输出预算。 |
| [Unified Exec 的职责与状态](https://github.com/openai/codex/blob/main/codex-rs/core/src/unified_exec/mod.rs) | 进程存储、输出上限、一次执行与继续交互的区分。审批和沙箱部分不纳入。 |
| [进程及输出管理](https://github.com/openai/codex/blob/main/codex-rs/core/src/unified_exec/process.rs) | 持续读取、待取输出与历史输出的区别、输出排空、交互串行化。 |
| [补丁工具处理](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/apply_patch.rs) | 补丁是独立文件修改能力，具有解析、执行与变化报告。 |
| [补丁语法](https://github.com/openai/codex/blob/main/codex-rs/core/assets/tools/apply_patch.lark) | Begin/End Patch、Add/Delete/Update File、Move to、上下文与 End of File 标记。 |
| [补丁库](https://github.com/openai/codex/blob/main/codex-rs/apply-patch/src/lib.rs) | 补丁动作、变化结果、工作目录、保留行尾等设计参考；该库当前依赖执行服务器相关抽象，不作为一个零成本可直接引用的独立解析器。 |
| [本地图像工具](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/handlers/view_image.rs) | 检查模型能力、读取并验证图像、用 `FunctionCallOutputBody::ContentItems` 回传 `InputImage`、日志排除图片数据 URL。 |

本方案另外选择：任务级而非长期共享的进程会话；首版只做管道标准输入交互；`apply_patch` 使用 JSON 字段；`write_stdin` 增加明确的关闭输入与终止意图；资源上限按本阶段需要集中设定。上述内容属于本阶段设计，不称为 Codex 的默认配置。

<a id="platform"></a>
## Windows、依赖与协议依据

[Windows Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)说明进程组管理，以及最后一个 Job 句柄关闭时终止关联进程的选项。本阶段只使用生命周期管理能力，不使用访问限制或隔离策略。

[process-wrap 的 Tokio Windows 接口](https://docs.rs/process-wrap/latest/x86_64-pc-windows-msvc/process_wrap/tokio/index.html)与[库说明](https://docs.rs/process-wrap/latest/process_wrap/)提供安全的 JobObject、KillOnDrop、CreationFlags 封装。选用前在实际 Windows 构建环境确认 feature、嵌套 Job 与关闭行为；这项依赖承担当前确有需要的进程树回收，不为未来平台预留层次。

[Known Folder 标识](https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid)将 Downloads 定义为当前用户的已知文件夹，默认路径为 `%USERPROFILE%\Downloads`；[SHGetKnownFolderPath](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shgetknownfolderpath)用于取得实际位置。考题优先解析实际位置，不能把 `\Downloads` 解释成当前盘根目录。

[Responses 创建请求](https://developers.openai.com/api/reference/resources/responses/methods/create)与[Function calling](https://developers.openai.com/api/docs/guides/function-calling)支持 `function_call_output.output` 使用包含图片的内容数组。本阶段还须通过当前账号、当前选定模型的真实请求确认这条路径；服务返回的实际错误按现有模型错误机制处理。

<a id="template"></a>
## 实验模板和 UI / BRP 库

[模板生成配置](https://github.com/slc90/bevy_app_template/blob/main/cargo-generate.toml)、[应用入口](https://github.com/slc90/bevy_app_template/blob/main/app/src/main.rs)和[应用配置](https://github.com/slc90/bevy_app_template/blob/main/app/src/config.rs)给出生成项目、Widgetry 窗口、可执行文件旁配置路径和 `brp_enabled` 开关。考题利用这些现有能力，不修改模板源仓库。

[模板 AGENTS.md](https://github.com/slc90/bevy_app_template/blob/main/AGENTS.md)与[GUI 验证规则](https://github.com/slc90/bevy_app_template/blob/main/rules/gui-debugging.md)说明实验项目原有的 BRP、测试和独立 Reviewer 要求。获准修改的是生成项目中的强制 BRP 约束及相关表述；其他规则不会被一并取消。

[Widgetry 文档](https://github.com/slc90/bevy_widgetry/blob/main/docs/README.md)给出 BSN 构造、公开组件与状态操作的使用原则。[BRP 文档](https://github.com/slc90/bevy_brp/blob/main/README.md)说明 MCP 与应用 Runtime 的区分、输入和截图能力，以及 Main/Render 端口关系。两库只作为使用和验证依据，不修改源码，也不借文档变化升级当前应用的既定依赖组合。

[cargo-generate 使用说明](https://cargo-generate.github.io/cargo-generate/)与[生成参数](https://docs.rs/cargo-generate/latest/cargo_generate/struct.GenerateArgs.html)说明基于 Git 模板生成项目、名称和目标路径、覆盖选项等。考题由被测模型检查本机 `cargo generate --help`，在 Downloads 下创建新的子目录，不对 Downloads 本身使用原地初始化或覆盖。
