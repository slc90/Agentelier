# 交付 MCP Server 管理窗口

## 目标

完成 B 区 MCP 按钮打开的管理窗口，让用户查看、添加和维护 MCP Server 配置，并查看真实连接状态和能力列表。

## Server 列表与操作

每个 Server 显示名称、传输方式、启用状态、连接状态和能力摘要。提供添加、编辑、启用/禁用、删除、查看连接错误，以及查看 Tools / Resources / Prompts 的入口。

配置状态、启用状态和连接状态分别表达。配置保存成功不代表连接成功；连接失败时保留配置并显示真实错误。

## 传输配置

**STDIO** 配置名称、可执行命令、参数、工作目录和允许传入的环境变量名称。

**Streamable HTTP** 配置名称、Endpoint URL 和当前支持的认证参数。

添加 MCP 是登记连接配置，不等于安装或下载第三方 Server。本阶段不提供 MCP 商店或自动安装器。

MCP 配置由 `crates/agent/mcp` 的 ConfigStore 管理，配置文件位于应用数据目录下的 `mcp_servers.json`。UI 不直接写配置文件，也不自行启动子进程或创建第二个 MCP Client。

## Session ToolCatalog 与上下文

配置变化影响之后创建的 Session；已有 Session 保持创建时冻结的 ToolCatalog。Server 启动本身不直接占用模型上下文；Tools 声明进入 Session ToolCatalog 后随模型请求发送。Resources / Prompts 不自动注入，由用户选择、预览并确认后才作为会话材料。

## Agent / Runtime / Host 影响

`crates/agent/mcp` 继续拥有 `ConfigStore`、Server 配置校验、连接状态和能力发现。需要为 UI 暴露添加、编辑、启用/禁用和删除的受控命令，以及对应配置快照和连接状态回执。

`crates/agent/runtime` 负责配置变更后的连接生命周期：按新配置启动、断开或重连 Server，并发布真实连接状态与 Tools / Resources / Prompts 能力。已有 Session 的 ToolCatalog 仍保持冻结。

`crates/ui/agent_host` 复用现有 Agent Runtime 消息入口转发管理命令和结果。当前架构没有独立 MCP Bridge Crate，不为 UI 管理窗口机械新增一个；若需要跨运行边界投影，沿用现有 AgentHost / Connection 约定。

## Widgetry 复用

Window、ListView、Collapsible、TextField、ComboBox / Segmented、ToggleButton、FileDialog、MessageBox 和 AgentelierButton。

## 工程边界

- 依赖方向保持 `ui → agent / bridge`、`bridge → agent`；Agent 不依赖 Bevy、UI 或 Bridge，Bridge 不拥有 Agent 业务状态。
- UI 使用 BSN、Scene 和 SceneList 构造与组合；Props 只初始化，运行状态由持久 Component / Resource 管理。
- 复用 `bevy_widgetry` 公开 API，不修改 Widgetry 上游、不访问其私有状态。
- 目标环境沿用 Windows x64、DX12 和 PowerShell 7。
- 不新增 Unit Test、Integration Test、Regression Test、Benchmark 或 Example 代码。GUI 行为使用真实应用和 BRP 验证；原生窗口行为在实际 Windows 窗口中观察。
## 完成条件与验收

1. 点击 B 区 MCP 按钮打开窗口，列表与 Agent ConfigSnapshot 一致。
2. Server 列表分别显示配置状态、启用状态和真实连接状态。
3. STDIO Server 可新增、编辑、保存、启用、禁用和删除。
4. STDIO 命令、参数、工作目录和环境变量按 Agent MCP 规则校验；命令不存在或目录无效时显示实际错误。
5. Streamable HTTP Server 可新增、编辑、保存、启用、禁用和删除。
6. HTTP Endpoint URL 遵循 Agent 现有规则；不接受含用户信息、查询或片段的 URL。
7. 认证值不显示在列表、普通日志或错误文本中；界面只显示安全摘要或环境变量名称。
8. 配置保存成功但 Server 启动失败时，配置保留，连接状态显示失败原因。
9. 启用后连接管理器按配置连接；禁用后按 Agent 生命周期断开，不影响其他 Server。
10. 编辑配置后，旧连接按配置变更回执收尾，新连接状态独立显示；保存前不显示新配置已生效。
11. 删除 Server 后配置列表移除对应项；已有 Session 的冻结 ToolCatalog 不静默绑定同名新 Server。
12. 新建 Session 使用最新启用 Server 的 ToolCatalog；已有 Session 不因窗口变化而重写工具目录。
13. Tools、Resources、Prompts 能力列表来自 Agent MCP 实际发现结果。
14. MCP Tools 声明进入新 Session 模型请求；Server 启动但未进入 ToolCatalog 时不额外占用上下文。
15. Resources / Prompts 不自动进入上下文；只有用户选择、预览和确认后才加入会话材料。
16. 连接断开、重连、超时、协议错误和能力发现失败时显示真实状态，配置保留。
17. MCP 认证不复用 ChatGPT OAuth 凭据；敏感环境变量和认证值不进入普通日志。
18. 使用真实 STDIO MCP Server 验收连接、能力发现和断开；使用真实 Streamable HTTP MCP Server 验收 HTTP 传输，不用 BRP HTTP 端口代替 MCP HTTP 端点。
19. 使用 BRP 验收列表、表单、启用/禁用和窗口交互，结合 Agent 回执确认连接状态。
