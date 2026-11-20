# Codex OAuth 与模型路由

Codex OAuth 是 Agentelier 当前的登录来源。用户从账号区选择由 Codex 客户端管理的 `auth.json`。Account 交付同一次读取中的主体、工作区和 Access Token。配置只保存路径，断开来源后回到未连接状态。

模型目录、连接验证、聊天、原生异步工具及后台记忆都通过同一个 Account 获取凭据。HTTP/SSE 与 WebSocket 请求访问 Codex Responses 服务。新建 `gpt-6-astra` 会话使用 WebSocket 接入原生异步。会话冻结的主体、工作区及工具目录决定是否可以继续旧请求。

历史 SIWC 会话保留附件、工具结果、归档协议和展示。缺少当前 Codex 身份绑定的记录可通过会话侧栏浏览。继续执行需要已经匹配当前主体和工作区的 Codex 存档。数据目录里的历史 SIWC 凭据维持原位。

图片生成设计复用当前任务身份和 Function Tool 派发。后续媒体请求应接受已冻结的执行身份，并沿用来源失效和持久化收尾边界。[认证运行](../modules/authentication.md) · [模型请求](../modules/model-request.md) · [图片生成设计](02-image-generation.md)
