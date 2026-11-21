# 独立执行、资源准入与 Runtime 驻留

每个 Agent 有独立 Turn Runner 和模型执行循环，但所有 Agent 受同一个 Session 级 Agent Control 管理。Runtime 负责创建、启动、持有和回收执行句柄；Conversation 保存 Agent 的业务状态和历史。不能把子 Agent 当成普通异步工具任务：工具任务没有独立模型循环、会话历史、消息收件箱和派生子树。

## 独立执行与会话所有权

子 Agent 在共享 Tokio Runtime 内运行各自的 Conversation、Turn Runner 和工具工作者。Agent Control 记录每个活动执行体的 Thread 和取消入口。子 Agent 的 Spawn 请求回到来源 Conversation 读取冻结调用步骤，持久登记由共同的 Session 所有者完成。Root 取消会传递到所属活动执行体，结束后按真实 Runner 和工具工作者回执收尾。

Root 和每个子 Agent 都有自己的 Conversation 与 Runner。一个 Agent 同一时刻最多有一个活动 Turn；不同 Agent 可以并发运行。Root 的前台用户任务仍只有一个活动入口，子 Agent 执行不占用 Root 的 Steer/Queue 前台槽。

Runtime 管理 Agent 的实际任务句柄、取消令牌、执行许可、驻留许可和 Mailbox 通知接收端。Conversation 不保存 JoinHandle、锁或 OS 资源。工具执行使用 Agent 自身的固定工作目录和权限快照；所有 Agent 默认共享会话工作目录。

## 三种独立限制

限制必须分开表达，因为它们保护不同资源：

- **总量限制**：一个 Root Session 生命周期内允许创建/登记的子 Agent 总数，防止无限派生。
- **并发执行限制**：同一时刻允许处于 Running 的子 Agent 数量，保护模型请求和工具资源。
- **树深度限制**：从 Root 到任意子 Agent 的最大派生层数，防止递归委派失控。

创建前先预留所有必要名额，创建失败时通过 RAII/显式回滚释放尚未提交的预留。持久化创建成功后，不能因调用方取消而把已登记身份当作不存在。容量许可的释放必须晚于真实执行结束回执。

Root 深度为 0，且不计入子 Agent 总量或并发。已登记身份持续占用总量，执行句柄占用并发直到 Runner、工具和存档收尾确认。Agent Control 在同一事件循环中串行准入，检查后到提交或回滚期间不会处理其他 Spawn 或 Follow-up。三个默认限制为总量 32、并发 4、深度 4，可分别用启动环境变量 `AGENTELIER_AGENT_TOTAL`、`AGENTELIER_AGENT_CONCURRENT`、`AGENTELIER_AGENT_DEPTH` 覆盖。

## 驻留管理

驻留限制控制内存中加载的子 Agent Runtime 数量，不改变 Agent Tree 总量。Root 始终驻留；正在执行、正在关闭、持有未确认资源或处于关键持久化提交过程中的 Agent 不可卸载。空闲且状态已保存的 Agent 可以成为卸载候选。

驻留管理要保护目标 Runtime 的唯一实例身份。卸载流程应取得独占驻留门闩，确认该 Agent 仍是 Registry 当前实例，停止接纳新执行，等待当前操作结束，将 Mailbox 未消费消息转移到持久层，保存有效配置与环境绑定，关闭 Runner/Runtime，最后从驻留表移除。任何阶段失败都保留明确错误和可恢复状态，不释放尚未确认的容量。

## 冷恢复

冷恢复按原 Agent ID 和父子路径恢复身份，不创建新 Agent。恢复顺序先恢复 Root/祖先链所需配置与身份，再恢复目标 Agent 的历史、有效配置、角色、模型、工作目录、工具绑定和待处理消息。旧 JoinHandle、CancellationToken、watch receiver 和进程句柄不可恢复；恢复后的新执行实例拥有新的 instance ID。

恢复只重建执行环境，不自动重复已经发生过的工具副作用。若持久化状态显示任务在崩溃时运行中但没有结果，必须形成中断/结果未知状态，由 Agent 按明确的历史事实继续或报告，而不是从头重放工具。

## 错误处理与公平性

并发容量不足时 Spawn 或 Follow-up 应返回可解释的资源忙状态，不能创建半个 Agent。驻留容量不足时可尝试回收符合条件的空闲子 Agent；不能卸载 Root、正在运行 Agent 或持有未确认提交的 Agent。等待容量期间不应无限占用创建预留，也不能发生两个请求同时认领同一个空闲名额。

共享工作目录允许一个 Agent 的文件修改被其他 Agent 看到。文件冲突由任务拆分和 Root 汇总处理；本次不提供 Worktree 隔离或自动 Git 合并。
