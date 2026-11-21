# 登录界面迁移与端到端验证

## 目标

将账号区从“选择 / 更换 / 断开外部 Codex `auth.json`”迁移为“使用 ChatGPT 登录 / 退出登录”的交互。界面继续通过共享 AgentHost 提交账号操作并消费 Bridge 转换后的账号事件，不直接调用 OAuth 网络逻辑或持有 Token。

## 登录交互

账号区未登录时显示「使用 ChatGPT 登录」。用户点击后，UI 提交一个带操作 ID 的登录命令；Login 启动本地回调服务并自动打开系统浏览器。等待授权期间，界面显示等待浏览器状态，并提供取消操作。成功后显示账号 Email 或主体、Workspace 和登录状态；失败时显示可理解的错误原因。浏览器自动打开失败但本地服务仍在等待时，应提供重新打开或复制授权 URL 的可用交互。

已登录时显示当前账号身份及「退出登录」。退出期间显示正在收尾状态，并禁用会与账号变更冲突的操作。用户取消登录、授权被拒绝、回调失败、Token 交换失败、凭据保存失败和登录超时均应结束当前操作并显示实际失败原因。

## 事件与状态投影

`login_ui` 继续持有面向交互的账号状态，`agent_host` 仍是唯一 Connection 所有者和业务消息接收者。`bridge/account` 将 Agent Account 事件转换为 UI 使用的安全摘要与状态，不传递 Access Token、Refresh Token 或 ID Token。

登录、取消、退出均使用操作 ID 关联用户操作及其最终结果，迟到事件不能覆盖当前操作状态。账号区保留现有 BSN / Widgetry 的布局和控件状态管理方式；按钮加载、禁用及状态文本由当前账号状态驱动。模型操作与账号操作冲突时，沿用共享宿主的活动操作限制。

移除外部凭据路径展示、文件选择、更换和断开操作。应用配置不再持久化 Codex `auth.json` 路径。数据目录设置仍保持原有职责，不与账号登录混为一谈。

## Codex Images 端到端验证

本次改造的关键行为不仅是 OAuth 登录成功，还要确认新凭据仍可调用 Codex 专用图片生成接口。使用已登录账号，从 Agent Account 经 CredentialClient 向 `llm_model` 交付凭据，调用 `https://chatgpt.com/backend-api/codex/images/generations`，并验证真实返回的 PNG 能正常解码和打开。

请求使用 `originator: agentelier`。图片生成继续使用 Codex OAuth 路线，不切换 SIWC。已有实验已使用备用端口 `1457` 完成真实授权和图片生成；端到端验证要确认正式账号链路仍保持这一能力。

## 文档同步

同步更新当前有效的 `docs/design/architecture.md`、`docs/design/modules/authentication.md`、`docs/design/modules/account-ui.md` 及受影响的模型请求设计。文档应描述自主 OAuth 凭据来源、Login / Account / Runtime / Bridge / UI 的真实职责、凭据持久化与生命周期，以及 Codex Images 的认证关系。

删除当前设计中“用户选择 Codex `auth.json`、Codex 客户端负责刷新、应用配置保存凭据路径”等已经失效的描述。`plans/` 中的历史材料不作为当前设计事实，也不需要为了这次改造回头整理历史方案。

## 完成条件

- 未登录账号区提供浏览器 OAuth 登录入口，不再显示外部文件选择器。
- 点击登录后自动打开浏览器；等待、取消、成功、失败和超时状态正确呈现。
- 登录成功显示账号身份与 Workspace，不向 UI / Bridge 传递 Token。
- 已登录状态提供退出登录入口，并正确显示退出收尾状态。
- 迟到的登录或退出事件不能覆盖新操作状态。
- 账号操作与模型操作的互斥仍遵循共享 AgentHost 现有机制。
- 使用正式 Account 凭据完成 Codex Images 请求，返回 PNG 能正常解码和打开。
- OAuth 授权 URL 与 Codex API 请求均使用 `originator=agentelier` / `originator: agentelier`。
- 通过实际应用运行和 BRP 验证账号区的登录入口、等待状态、成功状态及退出状态；涉及真实浏览器、系统窗口等 BRP 无法覆盖的行为，使用实际运行结果确认。
- 更新当前有效的架构和模块设计文档，删除外部 `auth.json` 作为当前认证来源的描述。
