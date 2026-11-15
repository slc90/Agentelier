# 应用宿主与程序入口

`app/src/main.rs` 只保留 Windows 子系统声明、Windows x64 平台检查和对 `agentelier_application::run()` 的调用。可执行包只依赖 `agentelier_application`，所有功能实现位于 `crates/`。

`agentelier_application` 是通用应用宿主。`run()` 建立日志、加载可执行文件旁的 `config/config.ron`、装配 Bevy、窗口、Asset、缩放、可选 BRP 与 LoginUiPlugin，运行主循环并返回进程退出码。配置的完整替换提交、启动诊断及整体退出协调由该库内部模块负责，不作为额外公共接口开放。

保存配置时，`data_directory` 通过 `bevy_widgetry_path_display::simplify_path` 将扩展盘符和 UNC 路径转换为无 `\\?\` 前缀的普通路径，其他命名空间保持原样；内存中的规范化路径继续保留。已有带前缀的配置仍可读取，再次保存时使用该转换。无法表示为 Unicode 的路径返回序列化错误，不使用有损显示文字保存。

目录、账号交互归 `agentelier_login_ui`，模型准备与验证交互归 `agentelier_chat_ui`。`agentelier_agent_host` 独占公共 Connection 并分发业务消息，登录 UI 管理浏览器和剪贴板；application 提供数据环境槽、配置保存及页面 Scene 装配，保存 AgentHost 的同一运行生命周期句柄。OAuth 与文件持久化归 Login，账号状态和凭据协调归 Account，模型网络归 LLM Model，执行调度与取消回收归 Runtime。

关闭时宿主先通过 AgentHost 停止并等待账号/模型运行结束，再等待辅助 Job、刷新日志并释放数据环境。主循环返回后保留兜底收尾。普通关闭继续保留账号会话，明确退出登录才清空会话。日志与配置的位置继续相对于可执行文件，不随源码或数据目录迁移。

依赖方向与职责关系见[总体架构](../design/architecture.md)，界面与生命周期接口见[登录与目录设置界面](login-ui.md)。
