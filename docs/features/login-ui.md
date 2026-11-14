# 登录启动界面

`agentelier_login_ui` 使用 Bevy BSN 和 Widgetry 构造数据目录启动界面。当前只实现目录阶段，不提供账号认证或设置页。

宿主安装现有 Bevy、应用 Asset、窗口和日志体系，再通过 `LoginUiPlugin::new` 传入已保存目录、`Arc<OnceLock<DataEnvironment>>` 及线程安全的目录保存操作。将 `LoginUiHost` 挂到 Widgetry 窗口的内容节点；同一插件实例只接纳一个宿主。

未保存路径时显示 Widgetry `PickDirectory`，明确确认后才检查。已有路径直接检查；失败界面显示路径及原因，提供重试、重新选择和取消。目录检查与保存操作在共享 JobManager 的后台线程执行。保存回调由宿主拥有，不能访问 ECS；应完整提交配置，并通过 `io::Result` 返回真实失败。

新路径保存成功或旧路径检查成功后，环境写入共享单次初始化槽；任务真实完成后发布 `DataDirectoryReady`。宿主据此更新界面和配置投影。取消选择或失败恢复会请求正常退出，关闭窗口也沿宿主退出流程执行。

宿主必须在主循环结束后停止并等待共享 JobManager 的所有任务结束，再关闭日志，最后释放共享环境槽。界面不安装第二套日志或事件循环，不把凭据内容放入反射状态或事件。应用装配见[总体架构](../design/architecture.md)，存放与错误契约见[数据目录与启动](../design/mechanisms/application-data.md)。
