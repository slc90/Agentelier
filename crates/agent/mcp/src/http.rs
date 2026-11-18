use crate::{
    connections::{Failure, Update, serve},
    settings::endpoint_url,
};
use reqwest::{Client, StatusCode, redirect::Policy};
use rmcp::transport::{
    DynamicTransportError, StreamableHttpClientTransport,
    streamable_http_client::{StreamableHttpClientTransportConfig, StreamableHttpError},
};
use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_READ_TIMEOUT: Duration = Duration::from_secs(75);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);

const MAX_BEARER_BYTES: usize = 16 * 1024;

pub(crate) async fn run(
    endpoint: String,
    bearer_environment: Option<String>,
    stop: CancellationToken,
    publish: impl Fn(Update),
) -> Result<(), Failure> {
    let url = endpoint_url(&endpoint).ok_or(Failure {
        stage: "配置",
        reason: "MCP HTTP 端点无效",
    })?;
    let mut builder = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(IDLE_READ_TIMEOUT);
    if url.host_str() == Some("localhost") {
        builder = builder.resolve_to_addrs(
            "localhost",
            &[
                SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
                SocketAddr::from((Ipv6Addr::LOCALHOST, 0)),
            ],
        );
    }
    let client = builder.build().map_err(|_| Failure {
        stage: "启动",
        reason: "无法构造 MCP HTTP 客户端",
    })?;
    let mut config = StreamableHttpClientTransportConfig::with_uri(url.to_string())
        .max_sse_event_size(crate::MAX_WIRE_MESSAGE_BYTES)
        .max_concurrent_requests(2)
        .control_request_timeout(CONTROL_TIMEOUT)
        .reinit_on_expired_session(false);
    if let Some(name) = bearer_environment {
        let token = std::env::var(name).map_err(|_| Failure {
            stage: "认证",
            reason: "未取得配置的 MCP Bearer 凭据环境变量",
        })?;
        if token.is_empty()
            || token.len() > MAX_BEARER_BYTES
            || !token.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(Failure {
                stage: "认证",
                reason: "配置的 MCP Bearer 凭据为空、过长或包含无效字符",
            });
        }
        config = config.auth_header(token);
    }
    serve(
        StreamableHttpClientTransport::with_client(client, config),
        &stop,
        publish,
    )
    .await
}

pub(crate) fn transport_failure(error: &DynamicTransportError) -> Option<Failure> {
    let error = error
        .error
        .downcast_ref::<StreamableHttpError<reqwest::Error>>()?;
    let (stage, reason) = match error {
        StreamableHttpError::AuthRequired(_) => ("认证", "HTTP 401，端点需要有效的 MCP 凭据"),
        StreamableHttpError::InsufficientScope(_) => ("认证", "HTTP 403，MCP 凭据权限不足"),
        StreamableHttpError::Client(error) if error.is_timeout() => ("连接", "MCP HTTP 请求超时"),
        StreamableHttpError::Client(error) if error.is_connect() => {
            ("连接", "无法连接配置的 MCP HTTP 端点")
        }
        StreamableHttpError::Client(error) if error.status() == Some(StatusCode::UNAUTHORIZED) => {
            ("认证", "HTTP 401，端点需要有效的 MCP 凭据")
        }
        StreamableHttpError::Client(error) if error.status() == Some(StatusCode::FORBIDDEN) => {
            ("认证", "HTTP 403，MCP 凭据权限不足")
        }
        StreamableHttpError::UnexpectedServerResponse(value) if value.starts_with("HTTP 401 ") => {
            ("认证", "HTTP 401，端点需要有效的 MCP 凭据")
        }
        StreamableHttpError::UnexpectedServerResponse(value) if value.starts_with("HTTP 403 ") => {
            ("认证", "HTTP 403，MCP 凭据权限不足")
        }
        StreamableHttpError::UnexpectedServerResponse(value) if value.starts_with("HTTP 3") => {
            ("连接", "MCP HTTP 端点返回重定向，目标已拒绝")
        }
        StreamableHttpError::HttpStatus(_) => ("连接", "MCP HTTP 端点返回失败状态码"),
        StreamableHttpError::SessionExpired => ("连接", "MCP HTTP 会话已过期，需要重新连接"),
        StreamableHttpError::ResponseBodyTooLarge { .. } => ("协议", "MCP HTTP 响应体超过上限"),
        StreamableHttpError::UnexpectedEndOfStream | StreamableHttpError::Sse(_) => {
            ("连接", "MCP HTTP 流提前结束或无法读取")
        }
        _ => ("协议", "MCP HTTP 请求失败或响应格式无效"),
    };
    Some(Failure { stage, reason })
}
