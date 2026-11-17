use crate::model::{
    ModelFailure, ModelFailureKind, diagnostic, http_failure, network, protocol, request_id,
    service_failure,
};
use crate::protocol::{ProtocolItem, ProtocolOutput};
use agentelier_account::AuthenticatedRequest;
use reqwest::Response;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::time::{Instant, timeout_at};

pub(crate) const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct OutputText {
    pub content_index: usize,
    pub text: String,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct OutputMessage {
    pub id: String,
    pub output_index: usize,
    pub request_index: u32,
    pub phase: Option<String>,
    pub content: Vec<OutputText>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionCall {
    pub id: String,
    pub output_index: usize,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}
pub struct GenerationResult {
    pub calls: Vec<FunctionCall>,
    pub messages: Vec<OutputMessage>,
    pub output: ProtocolOutput,
    pub usage: Option<crate::ResponseUsage>,
    pub response_id: Option<String>,
}
pub enum StreamEvent {
    Usage {
        response_id: Option<String>,
        usage: crate::ResponseUsage,
    },
    FunctionCall(FunctionCall),
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
        request_index: 0,
        phase,
        content,
    })
}
fn function_call(item: &Value, output_index: usize) -> Result<FunctionCall, ModelFailure> {
    let call = FunctionCall {
        id: field(item, "id")?.to_owned(),
        output_index,
        call_id: field(item, "call_id")?.to_owned(),
        name: field(item, "name")?.to_owned(),
        arguments: field(item, "arguments")?.to_owned(),
    };
    if call.id.is_empty() || call.call_id.is_empty() || call.name.is_empty() {
        return Err(protocol("工具调用缺少有效身份或名称"));
    }
    Ok(call)
}

fn completed(
    response: &Value,
    completed_reasoning: &HashMap<String, Value>,
    observed: &HashMap<usize, Value>,
) -> Result<GenerationResult, ModelFailure> {
    if field(response, "status")? != "completed" {
        return Err(protocol("完成事件的响应状态不一致"));
    }
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("完成响应缺少输出数组"))?;
    let mut messages = Vec::new();
    let mut calls = Vec::new();
    let mut item_ids = HashSet::new();
    let mut call_ids = HashSet::new();
    let mut protocol_items = Vec::new();
    for (index, item) in output.iter().enumerate() {
        let id = field(item, "id")?;
        if id.is_empty() || !item_ids.insert(id) {
            return Err(protocol("模型输出项身份重复或为空"));
        }
        if let Some(previous) = observed.get(&index) {
            if field(previous, "id")? != id || field(previous, "type")? != field(item, "type")? {
                return Err(protocol("完成输出与增量身份冲突"));
            }
            if field(item, "type")? == "function_call"
                && (field(previous, "call_id")? != field(item, "call_id")?
                    || field(previous, "name")? != field(item, "name")?)
            {
                return Err(protocol("完成工具调用与增量身份冲突"));
            }
        }
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
            "function_call" => {
                let call = function_call(item, index)?;
                if !call_ids.insert(call.call_id.clone()) {
                    return Err(protocol("模型重复使用调用身份"));
                }
                protocol_items.push(ProtocolItem::FunctionCall {
                    raw: item.clone(),
                    call_id: call.call_id.clone(),
                    name: call.name.clone(),
                });
                calls.push(call);
            }
            _ => return Err(protocol("不支持该模型输出类型")),
        }
    }
    for (index, previous) in observed {
        if output
            .get(*index)
            .is_none_or(|item| item.get("id") != previous.get("id"))
        {
            return Err(protocol("完成输出丢失已观察的输出项"));
        }
    }
    Ok(GenerationResult {
        calls,
        messages,
        output: ProtocolOutput(protocol_items),
        usage: crate::ResponseUsage::parse(response)?,
        response_id: response
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
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
    let mut observed: HashMap<usize, Value> = HashMap::new();
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
            if matches!(
                kind,
                "response.completed" | "response.failed" | "response.incomplete" | "error"
            ) {
                let response = event.get("response").unwrap_or(&event);
                if let Some(usage) = crate::ResponseUsage::parse(response)? {
                    publish(
                        progress,
                        StreamEvent::Usage {
                            response_id: response
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|id| !id.is_empty())
                                .map(str::to_owned),
                            usage,
                        },
                    )
                    .await?;
                }
            }
            match kind {
                "response.completed" => {
                    return completed(
                        event
                            .get("response")
                            .ok_or_else(|| protocol("完成事件缺少响应对象"))?,
                        &completed_reasoning,
                        &observed,
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
                    let output_index = index(&event, "output_index")?;
                    if let Some(previous) = observed.get(&output_index)
                        && (field(previous, "id")? != field(item, "id")?
                            || field(previous, "type")? != field(item, "type")?)
                    {
                        return Err(protocol("模型输出项身份冲突"));
                    }
                    if let Some(previous) = observed.get(&output_index)
                        && field(item, "type")? == "function_call"
                        && (field(previous, "call_id")? != field(item, "call_id")?
                            || field(previous, "name")? != field(item, "name")?)
                    {
                        return Err(protocol("工具调用身份在接收期间变化"));
                    }
                    if progress.is_none() && field(item, "type")? == "function_call" {
                        return Err(protocol("连接验证未声明工具，不能处理工具输出"));
                    }
                    observed.insert(output_index, item.clone());
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
                        "function_call" => {
                            publish(
                                progress,
                                StreamEvent::FunctionCall(function_call(item, output_index)?),
                            )
                            .await?
                        }
                        _ => return Err(protocol("不支持该模型输出类型")),
                    }
                }
                "response.function_call_arguments.delta"
                | "response.function_call_arguments.done" => {
                    request.mark_output_started();
                    let output_index = index(&event, "output_index")?;
                    let item = observed
                        .get_mut(&output_index)
                        .ok_or_else(|| protocol("工具参数缺少对应调用"))?;
                    if field(item, "type")? != "function_call"
                        || field(item, "id")? != field(&event, "item_id")?
                    {
                        return Err(protocol("工具参数身份不匹配"));
                    }
                    let arguments = if kind.ends_with(".delta") {
                        format!("{}{}", field(item, "arguments")?, field(&event, "delta")?)
                    } else {
                        field(&event, "arguments")?.to_owned()
                    };
                    item["arguments"] = Value::String(arguments);
                    publish(
                        progress,
                        StreamEvent::FunctionCall(function_call(item, output_index)?),
                    )
                    .await?;
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
