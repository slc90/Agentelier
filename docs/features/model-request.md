# 模型连接验证

主窗口提供“验证模型连接”。账号已登录且获得 `chatgpt.tokens.use.direct` 时可使用；未登录或缺少授权时提示先完成账号操作。每次点击发送固定输入“请简短回复，确认连接正常”，不携带上次响应，不保存聊天历史。

通过当前 OAuth Access Token 读取 `GET https://api.openai.com/v1/models`，保持服务顺序并选取首个 `visibility=list` 的模型。界面显示 `display_name`，请求使用 `slug`；目录无可显示模型或标识缺失时报错，没有默认模型、目录缓存或模型配置。

请求发送到公共 `POST https://api.openai.com/v1/responses`，仅包含模型、独立用户消息列表、`store=false` 和 `stream=true`。传输在 Agent 的 Tokio 中进行，不由 Bevy 或辅助 Job 发送认证 HTTP。

只有 `response.completed` 才算成功，完成对象中各输出项的 `output_text` 按服务顺序一次性展示，不重复追加增量文本。没有正文时明确说明已完成但没有文本内容。失败、未完成、显式错误及缺少完成事件的断流均为失败，部分正文不冒充成功。

同一时刻只允许一个验证操作；等待时按钮禁用，目录设置仍可使用。错误保留实际 HTTP 状态、结构化代码、诊断原因及可取得的请求标识。权限拒绝、使用限制、服务故障和网络问题不会直接退出账号；429 不自动重试或推断恢复时间。

每个 HTTP 请求在尚未开始输出且收到 401 时，最多使用同一账号的恢复凭据重发一次。取得目录后再次请求最新可用凭据并检查账号身份。已经开始模型输出后不自动重放。

退出登录和关闭窗口会取消当前模型操作，并等待其执行体真实结束。认证所有者已经开始的刷新与提交继续收敛；退出登录随后撤销并清理会话，普通关闭保留会话。旧结果不能覆盖退出后的界面。

JSON 和单个 SSE 事件最多 4 MiB，总事件流最多 16 MiB；模型 HTTP 请求总超时 120 秒、连接超时 15 秒，超出限制明确失败。本阶段仅实现单次纯文本连接验证，没有聊天、工具、逐字 UI 或自动重试服务故障。

实现关系见[单次模型请求](../design/modules/model-request.md)，运行与消息接口见[账号后台与业务桥接](auth-runtime.md)。

模型协议由 `agentelier_llm_model` 实现，通过 `agentelier_account` 获取凭据，由 `agentelier_agent_runtime` 管理任务。`agentelier_llm_model_bridge` 转换结果及取消/完成状态，`agentelier_bridge_connection` 负责操作关联、消息与唤醒。
