# 会话初始存档与单写入者

`agentelier_session_store` 管理本次固定数据目录中的会话文件，依赖 Conversation 定义的 InitialSession、StoredSettings 和 SessionSummary，不依赖 Bevy、Bridge、账号或模型执行器。Conversation 不反向依赖存储。

## 初始记录的提交边界

Runtime 从已有数据环境的凭据路径取得同一父目录；Store 仅写入该目录下的 `sessions/<uuid>/`，不读取认证文件，也不改变数据环境占用。会话 UUID 贯穿 Conversation、Bridge、UI 和取消控制，连接 operation ID 仍为本次进程内的独立身份。

Conversation 准备创建或 Reset，不立即替换当前会话。Store 工作者创建专属目录，将完整 JSONL 初始记录写入同目录临时文件，Flush 和 sync 后以不覆盖方式提交 `session.jsonl`。目录已存在时拒绝创建，不清理既有目录。初始记录提交前的失败只清理本次新建的空目录，清理失败保留实际错误。

每条记录有 `format = agentelier.session`、`version = 1`、`sequence` 和类型字段。初始 Created 记录序号为 1，由单写入者赋予；实际提交身份和序号通过回执交给 Runtime。尚未接入任务或队列记录的追加与恢复。

初始名称为“新会话”。权威记录提交后，再完整替换可重建的 `summary.json`，保存标题、创建时间、最后活动时间、最后序号及删除状态。摘要失败单独返回并记录错误，不能将已经提交的初始记录解释为不存在。当前尚不提供摘要扫描或重建入口。

Runtime 等待创建结果时继续消费账号、凭据及控制命令；等待期间拒绝启动另一项模型、会话或队列操作。成功后核对提交 UUID 和当前账号身份，再安装准备好的 Session；账号在等待期间变化时保留磁盘存档但不切换旧投影。初始记录不含账号所有权字段。

## 工作者的真实结束

Store 使用容量 16 的命令通道和独立阻塞 I/O 线程。命令接纳、权威记录提交及线程退出分别表达；等待者消失不取消已开始的操作。所有文件句柄在该次操作结束前同步并关闭。

普通关闭先停止接纳运行命令，回收模型执行，关闭 Store 的发送端并等待已接纳操作排空、真实 join 存储线程，再结束 Account 与 Tokio。收尾中的在途创建仍核对实际结果，存储失败向运行收尾传播。辅助 Job、日志和数据目录锁继续由原宿主顺序收尾。
