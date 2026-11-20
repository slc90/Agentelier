# 从数据目录进入 Codex 账号

应用启动先确定并独占本次数据目录。会话和长期记忆从该目录恢复。账号区使用应用配置中保存的外部 Codex `auth.json` 路径，未连接时提示选择文件。

- [数据目录与启动](mechanisms/application-data.md)说明目录选择、重启恢复及设置生效边界。
- [账号界面](modules/account-ui.md)说明文件选择、替换、断开及状态展示。
- [认证运行与凭据管理](modules/authentication.md)说明只读凭据、执行身份和来源变化时的任务收尾。
- [模型请求](modules/model-request.md)说明模型目录、连接验证、HTTP/SSE 与原生异步 WebSocket。
