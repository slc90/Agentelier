# 从数据目录进入 Codex 账号

应用启动先确定并独占本次数据目录。会话和长期记忆从该目录恢复。账号区提供 ChatGPT 浏览器授权入口，从本次数据目录恢复已有登录。

- [数据目录与启动](mechanisms/application-data.md)说明目录选择、重启恢复及设置生效边界。
- [账号界面](modules/account-ui.md)说明浏览器授权、取消、退出与账号状态展示。
- [认证运行与凭据管理](modules/authentication.md)说明浏览器 OAuth、本地凭据、执行身份和来源变化时的任务收尾。
- [模型请求](modules/model-request.md)说明模型目录、连接验证、HTTP/SSE 与原生异步 WebSocket。
