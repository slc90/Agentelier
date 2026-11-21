# 认证运行与凭据管理

Agentelier 使用用户选择的 Codex OAuth `auth.json`。Codex 客户端负责签发和更新文件。应用配置只保存所选绝对路径。数据目录自身的独占锁、已保存的会话和记忆具有独立生命周期。

## 选择与恢复

启动时，Runtime 在接管当前数据环境后读取配置中选定的文件。用户可以选取、替换或断开这一引用。选定路径的变化先原子提交配置，再用于后续读取。断开并完成旧执行收尾后，账号进入未连接状态，本地会话及记忆继续可读。数据目录下遗留的 SIWC `auth.json` 保留原位，目录检查和启动流程跳过它。

Login 以一次读取解析 Codex 文件，检查支持的 ChatGPT OAuth 模式、令牌结构、主体与工作区的一致性，并按 Access Token 的时间声明判断过期。本地字段提供文件一致性信息，远端服务负责验证实际授权。读取失败、过期及格式不符分别反馈。定期检查或后续凭据请求重新读取已选文件。

## 执行身份和授权变化

Account 是当前来源及凭据分发的所有者。CredentialClient 合并同一代次中并发的同类请求，通过 Runtime 交付当次可用的凭据。ExecutionIdentity 固定主体、工作区和来源代次。凭据版本还含 Access Token 摘要，用于判断 401 恢复是否取得新版本，以及 WebSocket 是否可以复用。

同一路径为原主体与工作区更新 Token 时，执行身份继续有效。输出开始前的 401 至多尝试一次重新读取。继续请求需要仍属于原主体和工作区的新版本，输出已经开始的请求保留实际失败。文件缺失或过期时，模型消费者等待有效凭据，用户可在 Codex 客户端恢复授权。

路径、可用性、主体或工作区变化时，Account 使旧执行身份和排队凭据请求失效，并暂停新凭据分发。Runtime 停止旧模型任务，回收工具和进程，交付原调用的已确认结果，并等待会话终态和队列暂停记录保存。确认收尾后才发布新状态并恢复接纳。工具真正执行前再次通过 CredentialClient 校验原任务身份。普通应用关闭取消并回收本地任务，应用配置继续引用已选文件。

## 模块边界

Login 负责数据环境、浏览器 OAuth、凭据解析和凭据值。Account 负责来源、共享请求和执行身份。Runtime 提供 Tokio 调度及收尾。LLM Model 通过同一 CredentialClient 发送目录、连接验证、聊天和长期记忆请求。Bridge 与 Bevy 只接收路径、安全摘要和状态错误，Token 留在执行侧。[账号界面](account-ui.md) · [模型请求](model-request.md)

Login 提供一次性的浏览器授权操作，使用回环监听与 Authorization Code + PKCE S256 交换凭据。授权 URL 使用实际绑定的端口，浏览器启动失败时可手动打开。回调核对本次 State，完成、失败、取消或超时后释放监听。授权结果包含 Token、Access Token 到期时间和账号摘要，供后续接入 Account。

外部凭据结构参考官方 Codex 的 [auth.json 存储](https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/storage.rs)与 [TokenData](https://github.com/openai/codex/blob/main/codex-rs/login/src/token_data.rs)。
