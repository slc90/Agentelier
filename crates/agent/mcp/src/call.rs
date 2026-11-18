use crate::{ServerId, discovery::Catalog};
use agentelier_tools::{ToolDefinition, ToolRegistration, ToolResult, ToolSource, ToolStatus};
use base64::Engine;
use rmcp::{
    RoleClient,
    model::{
        CallToolRequest, CallToolRequestParams, CallToolResult, ClientRequest, ContentBlock,
        ResourceContents, ServerResult,
    },
    service::{Peer, PeerRequestOptions, ServiceError},
};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const CANCEL_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(crate) struct Route {
    pub(crate) peer: Peer<RoleClient>,
    pub(crate) catalog: Arc<Catalog>,
    pub(crate) closed: CancellationToken,
}

#[derive(Clone)]
pub(crate) struct Gateway(tokio::sync::watch::Sender<BTreeMap<ServerId, Route>>);

impl Default for Gateway {
    fn default() -> Self {
        let (sender, _) = tokio::sync::watch::channel(BTreeMap::new());
        Self(sender)
    }
}

impl Gateway {
    pub(crate) fn update(&self, routes: BTreeMap<ServerId, Route>) {
        self.0.send_replace(routes);
    }

    pub(crate) fn register(&self, definition: ToolDefinition) -> ToolRegistration {
        let gateway = self.clone();
        let saved = Arc::new(definition.clone());
        ToolRegistration::new(definition, move |context, arguments, cancel| {
            let gateway = gateway.clone();
            let saved = saved.clone();
            let images = context.image_input();
            Box::pin(async move { gateway.call(&saved, arguments, images, cancel).await })
        })
    }

    async fn call(
        &self,
        definition: &ToolDefinition,
        arguments: &str,
        images: Option<bool>,
        cancel: &CancellationToken,
    ) -> ToolResult {
        let ToolSource::Mcp { server, tool, .. } = &definition.source else {
            return fail("mcp_route", "工具登记缺少 MCP 来源");
        };
        let Ok(arguments) = serde_json::from_str::<Map<String, Value>>(arguments) else {
            return fail("mcp_arguments", "MCP 工具参数须为有效 JSON 对象");
        };
        let route = self.0.borrow().get(server).cloned();
        let Some(route) = route else {
            return fail("mcp_offline", "原 MCP 服务器尚未连接或工具清单不可用");
        };
        if !route
            .catalog
            .tools
            .iter()
            .any(|current| current.same_declaration(definition))
        {
            return fail("mcp_contract", "当前 MCP 服务器或工具声明与会话快照不匹配");
        }
        let params = CallToolRequestParams::new(tool.clone()).with_arguments(arguments);
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        let deadline = tokio::time::sleep(CALL_TIMEOUT);
        tokio::pin!(deadline);
        let mut handle = tokio::select! {
            biased;
            _ = cancel.cancelled() => return cancelled("任务已停止，未提交 MCP 调用"),
            _ = route.closed.cancelled() => return disconnected(),
            _ = &mut deadline => return timed_out(),
            result = route.peer.send_cancellable_request(request, PeerRequestOptions::no_options()) => match result {
                Ok(handle) => handle,
                Err(error) => return protocol_error(error),
            }
        };
        let result = tokio::select! {
            biased;
            result = &mut handle.rx => result.unwrap_or(Err(ServiceError::TransportClosed)),
            _ = cancel.cancelled() => {
                let acknowledged = matches!(tokio::time::timeout(CANCEL_TIMEOUT, handle.cancel(Some("Agentelier task stopped".into()))).await, Ok(Ok(())));
                return cancelled(if acknowledged {
                    "已请求取消 MCP 调用；远端已产生的变化会保留"
                } else {
                    "已结束本地等待；无法确认服务器收到取消通知"
                });
            }
            _ = route.closed.cancelled() => return disconnected(),
            _ = &mut deadline => {
                if !matches!(tokio::time::timeout(CANCEL_TIMEOUT, handle.cancel(Some("Agentelier request timed out".into()))).await, Ok(Ok(()))) {
                    return ToolResult::error(ToolStatus::TimedOut, "mcp_timeout", "MCP 调用超时；无法确认服务器收到取消通知");
                }
                return timed_out();
            }
        };
        match result {
            Ok(ServerResult::CallToolResult(result)) => convert(result, images, cancel).await,
            Ok(_) => fail(
                "mcp_protocol",
                "MCP 服务器没有返回完整工具结果，交互式输入与服务器任务暂不可用",
            ),
            Err(error) => protocol_error(error),
        }
    }
}

fn fail(code: &str, message: &str) -> ToolResult {
    ToolResult::error(ToolStatus::Failed, code, message)
}

fn cancelled(message: &str) -> ToolResult {
    ToolResult::error(ToolStatus::Cancelled, "mcp_cancelled", message)
}

fn timed_out() -> ToolResult {
    ToolResult::error(
        ToolStatus::TimedOut,
        "mcp_timeout",
        "MCP 调用超时；未确认远端结果",
    )
}

fn disconnected() -> ToolResult {
    fail("mcp_offline", "原 MCP 连接已关闭；未确认远端结果")
}

fn protocol_error(error: ServiceError) -> ToolResult {
    match error {
        ServiceError::TransportSend(error) => crate::http::transport_failure(&error)
            .map_or_else(disconnected, |failure| fail("mcp_http", failure.reason)),
        ServiceError::TransportClosed => disconnected(),
        ServiceError::Timeout { .. } => timed_out(),
        ServiceError::Cancelled { .. } => cancelled("MCP 服务器取消了本次请求"),
        ServiceError::McpError(error) => fail(
            "mcp_rpc",
            &format!("服务器拒绝 MCP 调用，JSON-RPC 错误码 {}", error.code.0),
        ),
        _ => fail("mcp_protocol", "MCP 服务器返回不受支持的工具响应"),
    }
}

async fn convert(
    result: CallToolResult,
    images: Option<bool>,
    cancel: &CancellationToken,
) -> ToolResult {
    let mut content = Vec::new();
    let mut image = None;
    for item in result.content {
        content.push(match item {
            ContentBlock::Text(text) => json!({"type":"text","text":text.text}),
            ContentBlock::ResourceLink(resource) => json!({"type":"resource_link","uri":resource.uri,"name":resource.name,"mimeType":resource.mime_type}),
            ContentBlock::Image(block) if images != Some(false) && image.is_none() => {
                let marker = json!({"type":"image","mimeType":block.mime_type,"attachment":true});
                image = Some((content.len(), block));
                marker
            }
            ContentBlock::Image(block) => json!({"type":"image","mimeType":block.mime_type,"unavailable":
                if images == Some(false) {"当前模型不支持图片"} else {"当前工具结果只接收一张图片"}}),
            ContentBlock::Audio(block) => json!({"type":"audio","mimeType":block.mime_type,"unavailable":"暂不支持音频载荷"}),
            ContentBlock::Resource(block) => match block.resource {
                ResourceContents::TextResourceContents { uri, mime_type, text, .. } =>
                    json!({"type":"resource","uri":uri,"mimeType":mime_type,"text":text}),
                ResourceContents::BlobResourceContents { uri, mime_type, .. } =>
                    json!({"type":"resource","uri":uri,"mimeType":mime_type,"unavailable":"暂不支持二进制资源载荷"}),
                _ => json!({"type":"resource","unavailable":"不支持的资源内容"}),
            },
            _ => json!({"unavailable":"不支持的 MCP 内容块"}),
        });
    }
    let status = if result.is_error == Some(true) {
        ToolStatus::Failed
    } else {
        ToolStatus::Succeeded
    };
    let mut payload = json!({"isError":result.is_error.unwrap_or(false),"content":content});
    if let Some(structured) = result.structured_content {
        payload["structuredContent"] = structured;
    }
    let summary = payload.to_string();
    if summary.len() > agentelier_tools::MAX_TOOL_BODY_BYTES {
        return fail(
            "mcp_result_too_large",
            "MCP 工具结果超过文字预算；未截断或伪造完整结果",
        );
    }
    if let Some((index, block)) = image {
        let mut reason = "图片格式、编码或尺寸不受支持";
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&block.data) {
            let mut decoded =
                agentelier_tools::from_mcp_bytes(bytes, block.mime_type.clone(), summary, cancel)
                    .await;
            if decoded.output.image().is_some() {
                decoded.status = status;
                return decoded;
            }
            match decoded.status {
                ToolStatus::Cancelled => reason = "收到服务器结果后停止了解码",
                ToolStatus::TimedOut => reason = "图片解码超时",
                _ => {}
            }
        }
        payload["content"][index] =
            json!({"type":"image","mimeType":block.mime_type,"unavailable":reason});
        return ToolResult {
            status,
            output: payload.to_string().into(),
            cleanup_failed: false,
        };
    }
    ToolResult {
        status,
        output: summary.into(),
        cleanup_failed: false,
    }
}
