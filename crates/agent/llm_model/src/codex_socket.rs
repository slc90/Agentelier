use crate::model::{
    CODEX_RESPONSES_URL, ModelFailure, ModelFailureKind, acquire, authorize, http_failure,
    protocol, request_id,
};
use crate::protocol::{ProtocolItem, ProtocolOutput};
use crate::stream::{
    EventStream, GenerationResult, STREAM_IDLE_TIMEOUT, StreamEvent, consume_events,
};
use agentelier_account::{
    AuthenticatedRequest, CredentialClient, CredentialVersion, ExecutionIdentity, UsableCredentials,
};
use agentelier_tools::ToolCatalog;
use futures_util::{FutureExt, SinkExt, StreamExt};
use reqwest::header;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until, timeout, timeout_at};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::{
    Error, Message, handshake,
    protocol::{Role, WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

const MAX_CONNECTION_AGE: Duration = Duration::from_secs(55 * 60);
const PRECONNECTED_IDLE_LIMIT: Duration = Duration::from_secs(90);
const CONTINUATION_IDLE_LIMIT: Duration = Duration::from_secs(5 * 60);
struct Connection {
    socket: WebSocketStream<reqwest::Upgraded>,
    opened: Instant,
    request_id: Option<String>,
}

struct Continuation {
    connection: Connection,
    identity: ExecutionIdentity,
    version: CredentialVersion,
    cursor: Option<Cursor>,
}

pub(super) struct IdleSocket {
    take: CancellationToken,
    task: JoinHandle<Option<Continuation>>,
}

impl IdleSocket {
    pub(super) fn preconnect(credentials: CredentialClient) -> Self {
        Self::spawn(
            async move {
                let identity = credentials.execution().await.map_err(authentication)?;
                let current = acquire(&credentials).await?;
                identity.validate(&current).map_err(authentication)?;
                let client = super::generation::response_client(true)?;
                let connection = tokio::select! { biased;
                    _ = identity.invalidated() => return Err(authentication("本次预连的授权已失效")),
                    result = Connection::open(&client, &current) => result?,
                };
                tracing::debug!("Codex WebSocket 预连接已就绪");
                Ok(Continuation {
                    connection,
                    identity,
                    version: current.version().clone(),
                    cursor: None,
                })
            },
            PRECONNECTED_IDLE_LIMIT,
        )
    }

    fn spawn(
        open: impl Future<Output = Result<Continuation, ModelFailure>> + Send + 'static,
        idle_limit: Duration,
    ) -> Self {
        let take = CancellationToken::new();
        let taken = take.clone();
        let task = tokio::spawn(async move {
            let mut state = match open.await {
                Ok(state) => state,
                Err(failure) => {
                    tracing::debug!(kind = ?failure.kind, status = failure.status, "Codex WebSocket 预连接未能建立");
                    return None;
                }
            };
            let expiry =
                (state.connection.opened + MAX_CONNECTION_AGE).min(Instant::now() + idle_limit);
            let claimed = tokio::select! { biased;
                _ = state.identity.invalidated() => false,
                _ = sleep_until(expiry) => false,
                _ = taken.cancelled() => true,
                _ = state.connection.wait_idle() => false,
            };
            (claimed && state.connection.available()).then_some(state)
        });
        Self { take, task }
    }

    pub(super) fn is_pending_or_open(&self) -> bool {
        !self.task.is_finished()
    }

    async fn take(mut self) -> Option<Continuation> {
        self.take.cancel();
        match (&mut self.task).await {
            Ok(state) => state,
            Err(failure) => {
                tracing::error!(%failure, "Codex WebSocket 空闲任务异常结束");
                None
            }
        }
    }
}

impl Drop for IdleSocket {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Cursor {
    response_id: String,
    items: usize,
    digest: [u8; 32],
}

#[derive(Default)]
struct InputDigest(Sha256);

impl Write for InputDigest {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl InputDigest {
    fn append(&mut self, item: &Value) -> Result<(), ModelFailure> {
        serde_json::to_writer(&mut *self, item)
            .map_err(|_| protocol("无法计算 WebSocket 续接输入摘要"))?;
        self.0.update(b"\n");
        Ok(())
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

impl Cursor {
    fn matches(&self, input: &[Value]) -> Result<bool, ModelFailure> {
        let Some(prefix) = input.get(..self.items) else {
            return Ok(false);
        };
        let mut digest = InputDigest::default();
        for item in prefix {
            digest.append(item)?;
        }
        Ok(digest.finish() == self.digest)
    }
}

impl Connection {
    async fn open(
        client: &reqwest::Client,
        credentials: &UsableCredentials,
    ) -> Result<Self, ModelFailure> {
        let key = handshake::client::generate_key();
        let deadline = Instant::now() + super::generation::CONNECT_TIMEOUT;
        let response = timeout_at(
            deadline,
            authorize(client.get(CODEX_RESPONSES_URL), credentials)
                .header(header::CONNECTION, "Upgrade")
                .header(header::UPGRADE, "websocket")
                .header(header::SEC_WEBSOCKET_VERSION, "13")
                .header(header::SEC_WEBSOCKET_KEY, &key)
                .send(),
        )
        .await
        .map_err(|_| network("连接 Codex WebSocket 超时"))?
        .map_err(|_| network("无法建立 Codex WebSocket 连接"))?;
        if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
            return Err(http_failure(response).await);
        }
        let headers = response.headers();
        let expected = handshake::derive_accept_key(key.as_bytes());
        if !header_has_token(headers, header::UPGRADE, "websocket")
            || !header_has_token(headers, header::CONNECTION, "upgrade")
            || headers
                .get(header::SEC_WEBSOCKET_ACCEPT)
                .is_none_or(|value| value != &expected)
            || headers.contains_key(header::SEC_WEBSOCKET_EXTENSIONS)
            || headers.contains_key(header::SEC_WEBSOCKET_PROTOCOL)
        {
            return Err(protocol("Codex WebSocket 握手响应不符合协议"));
        }
        let id = request_id(&response);
        let upgraded = timeout_at(deadline, response.upgrade())
            .await
            .map_err(|_| network("等待 Codex WebSocket 升级超时"))?
            .map_err(|_| network("无法升级 Codex WebSocket 连接"))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(crate::stream::MAX_EVENT_BYTES))
            .max_frame_size(Some(crate::stream::MAX_EVENT_BYTES));
        Ok(Self {
            socket: WebSocketStream::from_raw_socket(upgraded, Role::Client, Some(config)).await,
            opened: Instant::now(),
            request_id: id,
        })
    }

    fn available(&mut self) -> bool {
        while let Some(message) = self.socket.next().now_or_never() {
            match message {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                _ => return false,
            }
        }
        true
    }

    async fn wait_idle(&mut self) {
        loop {
            match self.socket.next().await {
                Some(Ok(Message::Ping(_))) => {
                    if !matches!(
                        timeout(super::generation::CONNECT_TIMEOUT, self.socket.flush()).await,
                        Ok(Ok(()))
                    ) {
                        tracing::debug!("Codex WebSocket 空闲心跳回应失败");
                        break;
                    }
                    tracing::debug!("已回应 Codex WebSocket 空闲心跳");
                }
                Some(Ok(Message::Pong(_))) => {}
                _ => {
                    tracing::debug!("Codex WebSocket 空闲连接已结束");
                    break;
                }
            }
        }
    }
}

impl EventStream for Connection {
    async fn next_event(&mut self) -> Result<Option<String>, ModelFailure> {
        while let Some(message) = timeout(STREAM_IDLE_TIMEOUT, self.socket.next())
            .await
            .map_err(|_| network("Codex WebSocket 事件流空闲超时"))?
        {
            match message.map_err(|error| match error {
                Error::Capacity(_) => protocol("Codex WebSocket 事件超过允许大小"),
                _ => network("读取 Codex WebSocket 事件失败"),
            })? {
                Message::Text(text) => return Ok(Some(text.to_string())),
                Message::Close(_) => break,
                Message::Ping(_) => timeout(STREAM_IDLE_TIMEOUT, self.socket.flush())
                    .await
                    .map_err(|_| network("回应 Codex WebSocket 心跳超时"))?
                    .map_err(|_| network("无法回应 Codex WebSocket 心跳"))?,
                Message::Pong(_) => {}
                _ => return Err(protocol("Codex WebSocket 返回了非文本事件")),
            }
        }
        Ok(None)
    }
}

pub(super) async fn generate(
    previous: Option<IdleSocket>,
    client: &reqwest::Client,
    body: &[u8],
    tools: &ToolCatalog,
    credentials: &UsableCredentials,
    identity: &ExecutionIdentity,
    request: &mut AuthenticatedRequest,
    progress: &Sender<StreamEvent>,
) -> Result<(GenerationResult, IdleSocket), ModelFailure> {
    let mut payload: Value =
        serde_json::from_slice(body).map_err(|_| protocol("冻结的 WebSocket 请求不是有效 JSON"))?;
    let input = payload["input"]
        .as_array_mut()
        .ok_or_else(|| protocol("冻结的 WebSocket 请求缺少输入数组"))?;
    let mut digest = InputDigest::default();
    for item in input.iter() {
        digest.append(item)?;
    }
    let input_count = input.len();
    let previous = match previous {
        Some(socket) => socket.take().await,
        None => None,
    };
    let previous = previous.filter(|previous| {
        &previous.version == credentials.version()
            && previous.identity.validate(credentials).is_ok()
            && previous.connection.opened.elapsed() < MAX_CONNECTION_AGE
    });
    let reused_connection = previous.is_some();
    let (mut connection, cursor) = match previous {
        Some(previous) => (previous.connection, previous.cursor),
        None => (Connection::open(client, credentials).await?, None),
    };
    let previous_response = match cursor {
        Some(cursor) if cursor.matches(input)? => {
            input.drain(..cursor.items);
            Some(cursor.response_id)
        }
        _ => None,
    };
    let sent_items = input.len();
    let send_body = if let Some(id) = &previous_response {
        payload["previous_response_id"] = Value::String(id.clone());
        serde_json::to_string(&payload).map_err(|_| protocol("无法序列化 WebSocket 续接请求"))?
    } else {
        str::from_utf8(body)
            .map_err(|_| protocol("冻结的 WebSocket 请求不是有效 UTF-8"))?
            .to_owned()
    };
    tracing::debug!(
        previous_response_id = previous_response,
        input_items = sent_items,
        request_bytes = send_body.len(),
        reused_connection,
        "发送 Codex WebSocket 模型请求"
    );
    let message = Message::Text(send_body.into());
    timeout(STREAM_IDLE_TIMEOUT, connection.socket.send(message))
        .await
        .map_err(|_| network("发送 Codex WebSocket 请求超时"))?
        .map_err(|_| network("发送 Codex WebSocket 请求失败"))?;
    progress
        .send(StreamEvent::ResponseReceived {
            status: reqwest::StatusCode::SWITCHING_PROTOCOLS.as_u16(),
        })
        .await
        .map_err(|_| protocol("模型进度接收端已关闭"))?;
    let request_id = connection.request_id.clone();
    let result = consume_events(
        &mut connection,
        request_id,
        request,
        Some(progress),
        Some(tools),
    )
    .await?;
    let response_id = result
        .response_id
        .clone()
        .ok_or_else(|| protocol("Codex WebSocket 完成响应缺少身份"))?;
    let ProtocolOutput(output) = &result.output;
    let item_count = input_count + output.len();
    for item in output {
        let raw = match item {
            ProtocolItem::Compaction(item) => &item.0,
            ProtocolItem::Assistant { raw, .. }
            | ProtocolItem::Reasoning { raw, .. }
            | ProtocolItem::FunctionCall { raw, .. } => raw,
            _ => return Err(protocol("Codex WebSocket 完成响应包含本地历史项")),
        };
        digest.append(raw)?;
    }
    tracing::debug!(
        response_id,
        calls = result.calls.len(),
        async_calls = result
            .calls
            .iter()
            .filter(|call| call.async_flag == Some(true))
            .count(),
        "Codex WebSocket 响应接收完整"
    );
    let continuation = Continuation {
        connection,
        identity: identity.clone(),
        version: credentials.version().clone(),
        cursor: Some(Cursor {
            response_id,
            items: item_count,
            digest: digest.finish(),
        }),
    };
    Ok((
        result,
        IdleSocket::spawn(
            std::future::ready(Ok(continuation)),
            CONTINUATION_IDLE_LIMIT,
        ),
    ))
}

fn header_has_token(headers: &header::HeaderMap, name: header::HeaderName, token: &str) -> bool {
    headers
        .get_all(name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case(token))
}

fn network(message: &'static str) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Network, message)
}

fn authentication(failure: impl std::fmt::Display) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Authentication, failure.to_string())
}
