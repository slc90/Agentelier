# 账号后台与业务桥接

`AgentRuntime` 在独立线程驱动 Tokio，串行处理恢复、浏览器授权、重试保存和取消。网络与回环监听为异步；具体文件操作交给 spawn_blocking。文件缓存、待保存会话与 Token 仅在运行侧存在，不由 JobManager 代管。

`Connection::start(path, notify)` 在启动生产者前建立有界队列和通知能力。`restore()`、`login(consent)` 分配递增操作标识，只接纳一个在途账号操作。`try_receive()` 返回阶段事件、保存失败或实际终态；`retry_save(id)` 和 `cancel(id)` 作用于当前操作。保存失败不结束操作，重试保存不重新交换授权码。

命令容量为四，结果容量为八。消息先入队再通知应用；队列满、通道关闭、意外停止和通知失败形成明确错误，不将缺少终态当作正常完成。结果摘要和错误不引用 Agent 内部类型。BrowserReady 的 AuthorizationLink 不实现 Debug，宿主只能把它用于当前浏览器交互，不能复制进反射状态或日志。

`begin_shutdown()` 停止新命令和普通通知，取消当前监听与网络等待，已开始的文件提交继续结束。`is_finished()` 非阻塞检查线程，`finish()` join 并取得真实结果；线程只有在 Tokio 释放后才结束。宿主必须显式收尾处理错误，Drop 仅提供阻塞兜底。

Bridge、Agent、login 都不依赖 Bevy。宿主负责 Winit 通知、非敏感 UI 投影、系统默认浏览器和剪贴板；浏览器辅助任务继续归 Widgetry JobManager。首次打开及重新打开均通过 Windows Shell 将授权地址交给系统默认浏览器；启动失败时显示错误，保留重新打开与复制链接入口。当前不包含自动续期、退出登录或模型请求。
