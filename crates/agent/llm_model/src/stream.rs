use crate::model::{
    ModelFailure, ModelFailureKind, diagnostic, http_failure, network, protocol, request_id,
    service_failure,
};
use crate::protocol::{ProtocolItem, ProtocolOutput};
use agentelier_account::AuthenticatedRequest;
use reqwest::Response;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::time::{Instant, timeout_at};

pub(crate) const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct OutputText {
    pub content_index: usize,
    pub text: String,
}
#[derive(Clone)]
pub struct OutputMessage {
    pub id: String,
    pub output_index: usize,
    pub phase: Option<String>,
    pub content: Vec<OutputText>,
}
pub struct GenerationResult {
    pub messages: Vec<OutputMessage>,
    pub output: ProtocolOutput,
}
pub enum StreamEvent {
    Item(OutputMessage),
    Part {
        item_id: String,
        output_index: usize,
        part: OutputText,
    },
    Delta {
        item_id: String,
        output_index: usize,
        content_index: usize,
        text: String,
    },
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str, ModelFailure> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| protocol("模型事件缺少文本字段"))
}
fn index(value: &Value, name: &str) -> Result<usize, ModelFailure> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| protocol("模型事件缺少有效索引"))
}
fn text_part(part: &Value, content_index: usize) -> Result<OutputText, ModelFailure> {
    let text = match field(part, "type")? {
        "output_text" => field(part, "text")?,
        "refusal" => field(part, "refusal")?,
        _ => return Err(protocol("不支持该模型正文类型")),
    };
    Ok(OutputText {
        content_index,
        text: text.to_owned(),
    })
}
fn message(item: &Value, output_index: usize) -> Result<OutputMessage, ModelFailure> {
    if field(item, "role")? != "assistant" {
        return Err(protocol("模型输出消息角色无效"));
    }
    let phase = match item.get("phase") {
        None | Some(Value::Null) => None,
        Some(Value::String(phase)) => Some(phase.clone()),
        _ => return Err(protocol("模型消息阶段无效")),
    };
    let content = item
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("模型消息缺少内容数组"))?
        .iter()
        .enumerate()
        .map(|(index, part)| text_part(part, index))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(OutputMessage {
        id: field(item, "id")?.to_owned(),
        output_index,
        phase,
        content,
    })
}
fn completed(
    response: &Value,
    completed_reasoning: &HashMap<String, Value>,
) -> Result<GenerationResult, ModelFailure> {
    if field(response, "status")? != "completed" {
        return Err(protocol("完成事件的响应状态不一致"));
    }
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("完成响应缺少输出数组"))?;
    let mut messages = Vec::new();
    let mut protocol_items = Vec::new();
    for (index, item) in output.iter().enumerate() {
        match field(item, "type")? {
            "message" => {
                let message = message(item, index)?;
                protocol_items.push(ProtocolItem::Assistant {
                    raw: item.clone(),
                    phase: message.phase.clone(),
                });
                messages.push(message);
            }
            "reasoning" => {
                let mut raw = item.clone();
                if raw
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                    && let Some(done) = completed_reasoning.get(field(item, "id")?)
                    && let Some(encrypted) = done.get("encrypted_content")
                {
                    raw["encrypted_content"] = encrypted.clone();
                }
                let encrypted = raw
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.is_empty());
                protocol_items.push(ProtocolItem::Reasoning { raw, encrypted });
            }
            _ => return Err(protocol("不支持该模型输出类型；当前未启用工具执行")),
        }
    }
    Ok(GenerationResult {
        messages,
        output: ProtocolOutput(protocol_items),
    })
}
async fn publish(
    sender: Option<&Sender<StreamEvent>>,
    event: StreamEvent,
) -> Result<(), ModelFailure> {
    if let Some(sender) = sender {
        sender
            .send(event)
            .await
            .map_err(|_| protocol("模型增量接收端已关闭"))?;
    }
    Ok(())
}

#[derive(Default)]
struct Sse {
    line: Vec<u8>,
    data: String,
    after_cr: bool,
    first_line: bool,
    comment_activity: bool,
}
impl Sse {
    fn push(&mut self, byte: u8) -> Result<Option<String>, ModelFailure> {
        if byte == b'\n' && self.after_cr {
            self.after_cr = false;
            return Ok(None);
        }
        self.after_cr = byte == b'\r';
        if byte == b'\n' || byte == b'\r' {
            let bytes = std::mem::take(&mut self.line);
            let line = std::str::from_utf8(&bytes).map_err(|_| protocol("事件流包含无效 UTF-8"))?;
            let line = if !self.first_line {
                self.first_line = true;
                line.trim_start_matches('\u{feff}')
            } else {
                line
            };
            if line.is_empty() {
                if self.data.is_empty() {
                    return Ok(None);
                }
                self.data.pop();
                return Ok(Some(std::mem::take(&mut self.data)));
            }
            if line.starts_with(':') {
                self.comment_activity = true;
            }
            if let Some(data) = line.strip_prefix("data:") {
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
                self.data.push('\n');
            } else if line == "data" {
                self.data.push('\n');
            }
        } else {
            self.line.push(byte);
        }
        if self.line.len().saturating_add(self.data.len()) > MAX_EVENT_BYTES {
            return Err(protocol("模型事件超过允许大小"));
        }
        Ok(None)
    }
}

pub(crate) async fn consume(
    mut response: Response,
    request: &mut AuthenticatedRequest,
    progress: Option<&Sender<StreamEvent>>,
    idle_timeout: bool,
) -> Result<GenerationResult, ModelFailure> {
    let id = request_id(&response);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    // The subscription endpoint can return valid SSE without a Content-Type header.
    if !content_type.is_empty()
        && content_type
            .split(';')
            .next()
            .is_none_or(|mime| !mime.trim().eq_ignore_ascii_case("text/event-stream"))
    {
        let mime = diagnostic(content_type);
        let mut failure = http_failure(response).await;
        failure.kind = ModelFailureKind::Protocol;
        failure.message = format!("模型服务未返回 SSE 事件流（{mime}）：{}", failure.message);
        return Err(failure);
    }
    let mut parser = Sse::default();
    let mut completed_reasoning = HashMap::new();
    let mut received = 0usize;
    let mut deadline = Instant::now() + STREAM_IDLE_TIMEOUT;
    loop {
        let chunk = if idle_timeout {
            timeout_at(deadline, response.chunk())
                .await
                .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "模型事件流空闲超时"))?
        } else {
            response.chunk().await
        }
        .map_err(|error| {
            let mut failure = network(error);
            failure.request_id = id.clone();
            failure
        })?;
        let Some(chunk) = chunk else {
            break;
        };
        received = received.saturating_add(chunk.len());
        if received > MAX_STREAM_BYTES {
            return Err(protocol("模型事件流超过允许大小"));
        }
        for byte in chunk {
            let data = parser.push(byte)?;
            if std::mem::take(&mut parser.comment_activity) {
                deadline = Instant::now() + STREAM_IDLE_TIMEOUT;
            }
            let Some(data) = data else {
                continue;
            };
            let event: Value =
                serde_json::from_str(&data).map_err(|_| protocol("模型事件不是有效 JSON"))?;
            let kind = field(&event, "type")?;
            deadline = Instant::now() + STREAM_IDLE_TIMEOUT;
            match kind {
                "response.completed" => {
                    return completed(
                        event
                            .get("response")
                            .ok_or_else(|| protocol("完成事件缺少响应对象"))?,
                        &completed_reasoning,
                    );
                }
                "response.failed" => {
                    return Err(service_failure(
                        event.get("response").unwrap_or(&event),
                        None,
                        id,
                        "模型请求失败",
                    ));
                }
                "response.incomplete" => {
                    let reason = event
                        .pointer("/response/incomplete_details/reason")
                        .and_then(Value::as_str)
                        .unwrap_or("服务未提供原因");
                    let mut failure = ModelFailure::new(
                        ModelFailureKind::Incomplete,
                        format!("模型请求未完成：{}", diagnostic(reason)),
                    );
                    failure.request_id = id;
                    return Err(failure);
                }
                "error" => {
                    let status = event
                        .get("status_code")
                        .and_then(Value::as_u64)
                        .and_then(|status| u16::try_from(status).ok());
                    return Err(service_failure(&event, status, id, "模型服务返回错误事件"));
                }
                "response.output_item.added" | "response.output_item.done" => {
                    request.mark_output_started();
                    let item = event
                        .get("item")
                        .ok_or_else(|| protocol("输出事件缺少 item"))?;
                    match field(item, "type")? {
                        "message" => {
                            publish(
                                progress,
                                StreamEvent::Item(message(item, index(&event, "output_index")?)?),
                            )
                            .await?
                        }
                        "reasoning" => {
                            if kind == "response.output_item.done" {
                                completed_reasoning
                                    .insert(field(item, "id")?.to_owned(), item.clone());
                            }
                        }
                        _ => return Err(protocol("不支持该模型输出类型；当前未启用工具执行")),
                    }
                }
                "response.content_part.added" | "response.content_part.done" => {
                    request.mark_output_started();
                    let part = event
                        .get("part")
                        .ok_or_else(|| protocol("输出事件缺少 part"))?;
                    publish(
                        progress,
                        StreamEvent::Part {
                            item_id: field(&event, "item_id")?.to_owned(),
                            output_index: index(&event, "output_index")?,
                            part: text_part(part, index(&event, "content_index")?)?,
                        },
                    )
                    .await?;
                }
                "response.output_text.delta" | "response.refusal.delta" => {
                    request.mark_output_started();
                    publish(
                        progress,
                        StreamEvent::Delta {
                            item_id: field(&event, "item_id")?.to_owned(),
                            output_index: index(&event, "output_index")?,
                            content_index: index(&event, "content_index")?,
                            text: field(&event, "delta")?.to_owned(),
                        },
                    )
                    .await?;
                }
                _ => {}
            }
        }
    }
    let mut failure = ModelFailure::new(
        ModelFailureKind::Network,
        "连接中断：事件流结束前未收到 response.completed",
    );
    failure.request_id = id;
    Err(failure)
}
