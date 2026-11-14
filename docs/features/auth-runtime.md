# 账号后台与业务桥接

`AgentRuntime` 在独立线程驱动 Tokio，串行处理恢复、浏览器授权、重试保存和取消。网络与回环监听为异步；具体文件操作交给 spawn_blocking。文件缓存、待保存会话与 Token 仅在运行侧存在，不由 JobManager 代管。

`Connection::start(path, notify)` 在启动生产者前建立有界队列和通知能力。`restore()`、`login(consent)` 分配递增操作标识，只接纳一个在途账号操作。`try_receive()` 返回阶段事件、保存失败或实际终态；`retry_save(id)` 和 `cancel(id)` 作用于当前操作。保存失败不结束操作，重试保存不重新交换授权码。

命令容量为四，结果容量为八。消息先入队再通知应用；队列满、通道关闭、意外停止和通知失败形成明确错误，不将缺少终态当作正常完成。结果摘要和错误不引用 Agent 内部类型。BrowserReady 的 AuthorizationLink 不实现 Debug，宿主只能把它用于当前浏览器交互，不能复制进反射状态或日志。

`begin_shutdown()` 停止新命令和普通通知，取消当前监听与网络等待，已开始的文件提交继续结束。`is_finished()` 非阻塞检查线程，`finish()` join 并取得真实结果；线程只有在 Tokio 释放后才结束。宿主必须显式收尾处理错误，Drop 仅提供阻塞兜底。

Bridge、Agent、login 都不依赖 Bevy。宿主负责 Winit 通知、非敏感 UI 投影、系统默认浏览器和剪贴板；浏览器辅助任务继续归 Widgetry JobManager。首次打开及重新打开均通过 Windows Shell 将授权地址交给系统默认浏览器；启动失败时显示错误，保留重新打开与复制链接入口。当前不包含退出登录或模型请求。

## 共享凭据请求

`AgentRuntime::credentials()` 返回可克隆的 `CredentialClient`。`request()` 提交按需获取，`CredentialRequest::wait()` 异步等待；多个相同类型的在途请求共享一个结果。丢弃等待句柄仅结束调用方等待，不取消认证运行中的旋转。句柄不持有第二套 Token 缓存；结果通过不实现 Debug/Reflect 的私有通道返回运行侧消费者。

`recover(&mut AuthenticatedRequest, status)` 在原请求预算允许时提交一次恢复。同一登记已存在有效的新代次时直接复用；否则在允许的刷新时间内尝试续期。不同账号身份拒绝恢复；恢复预算保存在原请求上下文中，调用方必须在重发时继续持有该上下文。

`Connection::check_session()` 走相同共享请求路径，只把非敏感 `SessionStatus` 交给界面。此类事件使用保留标识 0，不占用浏览器操作标识，携带账号投影、Checking/Refreshing/Saving/SaveFailed/Idle 和安全错误分类；不会传递 Token。保存失败时再次检查先保存待提交记录，不重复网络交换。

关闭时停止接纳新请求，取消可取消的浏览器授权；已经开始的续期网络处理及提交继续结束。若仍有待提交续期或清理结果，再尝试一次提交，失败通过运行收尾明确报告。凭据客户端的克隆不会阻止认证线程退出；排队但未开始的请求在关闭后得到通道结束错误。持久化持续不可用时无法保证新凭据在进程退出后恢复，不报告保存成功。
