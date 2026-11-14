# 账号后台与业务桥接

`agentelier_agent_runtime::AgentRuntime` 在独立线程驱动 Tokio，将恢复、浏览器授权、重试保存和取消交给 `agentelier_account::Account` 串行处理。网络与回环监听为异步；具体文件操作交给 spawn_blocking。文件缓存、待保存会话与 Token 由 Account/Login 持有，不由 JobManager 代管。

`agentelier_bridge_connection::Connection::start(path, notify)` 在启动生产者前建立有界队列和通知能力。`restore()`、`login(consent)` 分配递增操作标识，只接纳一个在途账号操作。`try_receive()` 返回阶段事件、保存失败或实际终态；`retry_save(id)` 和 `cancel(id)` 作用于当前操作。保存失败不结束操作，重试保存不重新交换授权码。

命令容量为四，结果容量为八。消息先入队再通知应用；队列满、通道关闭、意外停止和通知失败形成明确错误，不将缺少终态当作正常完成。结果摘要和错误不引用 Agent 内部类型。BrowserReady 的 AuthorizationLink 不实现 Debug，宿主只能把它用于当前浏览器交互，不能复制进反射状态或日志。

`begin_shutdown()` 停止新命令和普通通知，取消当前监听与网络等待，已开始的文件提交继续结束。`is_finished()` 非阻塞检查线程，`finish()` join 并取得真实结果；线程只有在 Tokio 释放后才结束。宿主必须显式收尾处理错误，Drop 仅提供阻塞兜底。

Bridge、Agent、login 都不依赖 Bevy。`agentelier_agent_host::AgentHost` 独占 Connection，提供 Winit 通知、命令转交及执行收尾；公共接收系统只读取一次连接，将带 operation_id 的账号与模型业务事件分别转交 ECS 消息通道，并单独广播运行失败。login_ui 在公共接收之后消费对应业务消息，负责非敏感 UI 投影、系统默认浏览器和剪贴板；application 负责通用宿主装配与整体收尾，浏览器辅助任务继续归 Widgetry JobManager。首次打开及重新打开均通过 Windows Shell 将授权地址交给系统默认浏览器；启动失败时显示错误，保留重新打开与复制链接入口。模型 HTTP 和 SSE 由 `agentelier_llm_model` 实现，在 Runtime 的同一 Tokio 中运行。

## 共享凭据请求

`AgentRuntime::credentials()` 返回 Account 提供的可克隆 `CredentialClient`。`request()` 提交按需获取，`CredentialRequest::wait()` 异步等待；多个相同类型的在途请求共享一个结果。丢弃等待句柄仅结束调用方等待，不取消认证运行中的旋转。句柄不持有第二套 Token 缓存；结果通过不实现 Debug/Reflect 的私有通道返回运行侧消费者。

`recover(&mut AuthenticatedRequest, status)` 在原请求预算允许时提交一次恢复。同一登记已存在有效的新代次时直接复用；否则在允许的刷新时间内尝试续期。不同账号身份拒绝恢复；恢复预算保存在原请求上下文中，调用方必须在重发时继续持有该上下文。

`Connection::check_session()` 走相同共享请求路径，只把非敏感 `SessionStatus` 交给界面。此类事件使用保留标识 0，不占用浏览器操作标识，携带账号投影、Checking/Refreshing/Saving/SaveFailed/Idle 和安全错误分类；不会传递 Token。保存失败时再次检查先保存待提交记录，不重复网络交换。

关闭时停止接纳新请求，取消可取消的浏览器授权；已经开始的续期网络处理及提交继续结束。若仍有待提交续期或清理结果，再尝试一次提交，失败通过运行收尾明确报告。凭据客户端的克隆不会阻止认证线程退出；排队但未开始的请求在关闭后得到通道结束错误。持久化持续不可用时无法保证新凭据在进程退出后恢复，不报告保存成功。

## 退出控制与迟到结果

`Connection::sign_out()` 分配退出标识；本地清理失败后再次调用复用该标识重试。退出独立于原登录操作，Bridge 消费并隔离原操作和全局会话的中间结果，仅把当前退出阶段或终态交给界面；退出期间不接纳登录、恢复、会话检查或普通保存重试。

`AgentRuntime::sign_out(id)` 通过容量为一的专用控制通道提交退出，同步关闭 CredentialClient 入口并取消当前会话等待者的交付许可。当前浏览器等待被取消，已经开始的旋转和提交继续结束；所有者随后优先处理退出，丢弃尚未开始的旧命令，并以最新会话撤销和清理。旧 CredentialRequest 即使已经持有待取结果，也不能在退出后返回旧 Token；新的凭据入口仅在真实浏览器授权保存成功后重开。已经交付给业务的凭据由业务停止/取消路径结束使用，模型操作取消并回收后才继续退出清理。

退出成功以 SignedOut 事件提供安全账号投影及 Confirmed/Unconfirmed/NotApplicable 的远端结果。SignOutFailed 保持禁止新请求，允许重试同一份本地清理。通知失败不会中途丢弃必要凭据提交；已接纳的退出优先于普通关闭收敛。普通关闭只停止运行，不自动退出账号。


## 单次模型操作

`verify_model()` 分配独立操作 ID，接纳后阻止新的登录、恢复、检查和重复验证，直到实际终态；退出仍可发起。Agent 在同一 Tokio 中启动一个模型任务，认证所有者继续处理该任务的 CredentialClient 请求。Runtime 的 `ModelCompleted`/`ModelCancelled` 经过 LLM Model Bridge 转成 `ModelEventKind::Completed`/`Cancelled`；Completed 提供该 Bridge 自己的 ModelReply 或 ModelFailure（分类、HTTP 状态、代码、请求标识、诊断原因），Cancelled 表示执行体已实际结束。模型结果入队后沿相同 Winit 通知路径唤醒应用。

退出登录先取消并 join 当前模型任务，再撤销/清理会话；Bridge 隔离其迟到结果。关闭同样取消并等待模型任务，然后结束认证/Tokio。模型取消不会撤销认证所有者已经开始的刷新或丢弃待保存结果。目录和流协议见[模型连接验证](model-request.md)。

## 业务转换与连接封装

Account Bridge (`agentelier_account_bridge`) 提供账号摘要、授权地址及 AccountEventKind 的安全转换。LLM Model Bridge (`agentelier_llm_model_bridge`) 独立提供模型结果与错误分类转换。两者不执行账号或模型任务。

Connection 的 `try_receive()` 返回 `BridgeEvent { operation_id, kind }`，其中 `BridgeEventKind::Account(AccountEventKind)` 与 `BridgeEventKind::Model(ModelEventKind)` 分别携带对应业务类型。Connection 只管理投递、操作关联、互斥、唤醒和通信生命周期；不拥有账号会话或 UI 投影。

共享宿主在数据目录就绪后启动，沿用本次数据环境的凭据路径，不持有第二份目录占用或账号缓存。页面状态不拥有后台线程。application 保存 AgentHost 的收尾句柄，先 begin_shutdown、非阻塞确认 is_finished，再 finish 获取实际线程结果；登录 UI 清理自己的授权地址和浏览器任务，辅助 Job、日志及目录按原顺序结束。
