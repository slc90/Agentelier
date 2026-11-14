# 账号恢复后台与桥接

`agentelier_agent::AgentRuntime` 在独立线程中驱动 Tokio current-thread Runtime，通过容量为一的命令队列接收恢复操作标识，使用 `spawn_blocking` 调用 `agentelier_login::AuthFile`。文件组件和原始凭据只由运行体持有。结果发布失败、任务异常及线程异常以 `RuntimeFailure` 返回。

`agentelier_bridge::Connection::start(path, notify)` 在启动生产者之前建立容量为一的结果队列和通知能力。`restore()` 分配递增操作标识，只接受一个在途请求；`try_receive()` 非阻塞地返回安全的 `AccountEvent`，其消息不引用 Agent 的内部类型。队列满、接收端关闭、意外停止和通知失败都形成明确错误。通知回调返回 `false` 表示无法唤醒，不携带业务数据。

`begin_shutdown()` 停止新命令并关闭普通通知，已接收的有限读操作继续结束。宿主保留 Connection，随后调用 `finish()`，join 执行线程并取得真实运行结果。线程只有在 Tokio 释放后才返回。调用者必须显式收尾以处理错误；Drop 仅作为防止运行泄漏的阻塞收尾兜底，并记录无法返回给调用者的失败。

Bridge、Agent 和 login 都不依赖 Bevy。只有宿主把通知转换为 `WinitUserEvent::WakeUp`，并把安全业务结果投影到 `login_ui::AccountState`。本次没有 OAuth、刷新、退出登录、模型请求或通用事件总线。
