use crate::model::{
    CODEX_RESPONSES_URL, ModelFailure, ModelFailureKind, authorize, http_failure, protocol,
    request_id,
};
use crate::protocol::{ProtocolItem, ProtocolOutput};
use crate::stream::{
    EventStream, GenerationResult, STREAM_IDLE_TIMEOUT, StreamEvent, consume_events,
};
use agentelier_account::{
    AuthenticatedRequest, CredentialVersion, ExecutionIdentity, UsableCredentials,
};
use agentelier_tools::ToolCatalog;
use futures_util::{FutureExt, SinkExt, StreamExt};
use reqwest::header;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::{
    Error, Message, handshake,
    protocol::{Role, WebSocketConfig},
};

const MAX_CONNECTION_AGE: Duration = Duration::from_secs(55 * 60);
struct Connection {
    socket: WebSocketStream<reqwest::Upgraded>,
    opened: Instant,
    request_id: Option<String>,
}

pub(super) struct Continuation {
    connection: Connection,
    identity: ExecutionIdentity,
    version: CredentialVersion,
    cursor: Cursor,
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
        let response = timeout(
            super::generation::CONNECT_TIMEOUT,
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
        let upgraded = timeout(super::generation::CONNECT_TIMEOUT, response.upgrade())
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
}

impl EventStream for Connection {
    async fn next_event(&mut self) -> Result<Option<String>, ModelFailure> {
        timeout(STREAM_IDLE_TIMEOUT, async {
            while let Some(message) = self.socket.next().await {
                match message.map_err(|error| match error {
                    Error::Capacity(_) => protocol("Codex WebSocket 事件超过允许大小"),
                    _ => network("读取 Codex WebSocket 事件失败"),
                })? {
                    Message::Text(text) => return Ok(Some(text.to_string())),
                    Message::Close(_) => break,
                    Message::Ping(_) => self
                        .socket
                        .flush()
                        .await
                        .map_err(|_| network("无法回应 Codex WebSocket 心跳"))?,
                    Message::Pong(_) => {}
                    _ => return Err(protocol("Codex WebSocket 返回了非文本事件")),
                }
            }
            Ok(None)
        })
        .await
        .map_err(|_| network("Codex WebSocket 事件流空闲超时"))?
    }
}

pub(super) async fn generate(
    previous: Option<Continuation>,
    client: &reqwest::Client,
    body: &[u8],
    tools: &ToolCatalog,
    credentials: &UsableCredentials,
    identity: &ExecutionIdentity,
    request: &mut AuthenticatedRequest,
    progress: &Sender<StreamEvent>,
) -> Result<(GenerationResult, Continuation), ModelFailure> {
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
    let previous = previous.filter(|previous| {
        &previous.version == credentials.version()
            && previous.identity.validate(credentials).is_ok()
            && previous.connection.opened.elapsed() < MAX_CONNECTION_AGE
    });
    let (mut connection, cursor) = match previous
        .and_then(|mut previous| previous.connection.available().then_some(previous))
    {
        Some(previous) => (previous.connection, Some(previous.cursor)),
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
    Ok((
        result,
        Continuation {
            connection,
            identity: identity.clone(),
            version: credentials.version().clone(),
            cursor: Cursor {
                response_id,
                items: item_count,
                digest: digest.finish(),
            },
        },
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
