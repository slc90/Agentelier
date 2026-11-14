use agentelier_account::{AuthenticatedRequest, CredentialClient, UsableCredentials};
use reqwest::{Client, Response};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use std::{error::Error, fmt};

const MODELS_URL: &str = "https://api.openai.com/v1/models";
const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const INPUT: &str = "请简短回复，确认连接正常";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;
const MAX_DIAGNOSTIC_CHARS: usize = 2048;

#[derive(Clone)]
pub struct ModelInfo {
    pub slug: String,
    pub display_name: String,
    pub supported_reasoning_levels: Vec<String>,
    pub default_reasoning_level: Option<String>,
}

pub enum ModelRequest {
    Catalog,
    Verify {
        slug: String,
        display_name: String,
        effort: Option<String>,
    },
}

pub enum ModelResult {
    Catalog(Vec<ModelInfo>),
    Verified(ModelReply),
}

pub struct ModelReply {
    pub slug: String,
    pub effort: Option<String>,
    pub model: String,
    pub text: String,
}
#[derive(Clone, Copy, Debug)]
pub enum ModelFailureKind {
    Authentication,
    Permission,
    UsageLimit,
    Service,
    Network,
    Protocol,
    Incomplete,
}
#[derive(Clone, Debug)]
pub struct ModelFailure {
    pub kind: ModelFailureKind,
    pub status: Option<u16>,
    pub code: Option<String>,
    pub request_id: Option<String>,
    pub message: String,
}
impl ModelFailure {
    fn new(kind: ModelFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            code: None,
            request_id: None,
            message: message.into(),
        }
    }
    fn authentication(failure: impl fmt::Display) -> Self {
        Self::new(ModelFailureKind::Authentication, failure.to_string())
    }
}
impl fmt::Display for ModelFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(status) = self.status {
            write!(f, "；HTTP {status}")?;
        }
        if let Some(code) = &self.code {
            write!(f, "；{code}")?;
        }
        if let Some(id) = &self.request_id {
            write!(f, "；请求 {id}")?;
        }
        Ok(())
    }
}
impl Error for ModelFailure {}

pub async fn execute(
    credentials: CredentialClient,
    operation: ModelRequest,
) -> Result<ModelResult, ModelFailure> {
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "无法建立安全模型客户端"))?;
    let mut current = acquire(&credentials).await?;
    let ModelRequest::Verify {
        slug,
        display_name,
        effort,
    } = operation
    else {
        let mut catalog_request = AuthenticatedRequest::new(&current);
        let catalog = loop {
            let response = client
                .get(MODELS_URL)
                .bearer_auth(current.access_token())
                .send()
                .await
                .map_err(network)?;
            if response.status().is_success() {
                let status = response.status().as_u16();
                let id = request_id(&response);
                break read_json(response)
                    .await
                    .map_err(|failure| with_response(failure, status, id))?;
            }
            let failure = http_failure(response).await;
            if let Some(replacement) =
                recover(&credentials, &mut catalog_request, &current, &failure).await?
            {
                current = replacement;
            } else {
                return Err(failure);
            }
        };
        let models = catalog
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| protocol("模型目录缺少 models 数组"))?;
        let mut result = Vec::new();
        for model in models
            .iter()
            .filter(|model| model.get("visibility").and_then(Value::as_str) == Some("list"))
        {
            let slug = required_text(model, "slug", "可显示模型缺少 slug")?.to_owned();
            let display_name =
                required_text(model, "display_name", "可显示模型缺少 display_name")?.to_owned();
            let (supported_reasoning_levels, default_reasoning_level) =
                reasoning_capabilities(model)?;
            if result.iter().any(|entry: &ModelInfo| entry.slug == slug) {
                return Err(protocol("模型目录含有重复 slug"));
            }
            result.push(ModelInfo {
                slug,
                display_name,
                supported_reasoning_levels,
                default_reasoning_level,
            });
        }
        tracing::debug!(model_count = result.len(), "已读取当前账号模型目录");
        return Ok(ModelResult::Catalog(result));
    };
    if slug.trim().is_empty() {
        return Err(protocol("未选择模型"));
    }
    tracing::debug!(model = slug, effort, "使用显式选择的模型与推理设置验证连接");
    let mut payload = serde_json::json!({"model": slug, "input": [{"role": "user", "content": INPUT}], "store": false, "stream": true});
    if let Some(effort) = &effort {
        payload["reasoning"] = serde_json::json!({"effort": effort});
    }
    let mut request = AuthenticatedRequest::new(&current);
    loop {
        let response = client
            .post(RESPONSES_URL)
            .bearer_auth(current.access_token())
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&payload)
            .send()
            .await
            .map_err(network)?;
        let outcome = if response.status().is_success() {
            let status = response.status().as_u16();
            let id = request_id(&response);
            consume(response, &mut request)
                .await
                .map_err(|failure| with_response(failure, status, id))
        } else {
            Err(http_failure(response).await)
        };
        match outcome {
            Ok(text) => {
                return Ok(ModelResult::Verified(ModelReply {
                    slug,
                    effort,
                    model: display_name,
                    text,
                }));
            }
            Err(failure) => {
                if let Some(replacement) =
                    recover(&credentials, &mut request, &current, &failure).await?
                {
                    current = replacement;
                } else {
                    return Err(failure);
                }
            }
        }
    }
}
#[derive(Deserialize)]
struct ReasoningLevel {
    effort: String,
}

fn reasoning_capabilities(model: &Value) -> Result<(Vec<String>, Option<String>), ModelFailure> {
    let levels = match model
        .get("supported_reasoning_levels")
        .filter(|value| !value.is_null())
    {
        None => Vec::new(),
        Some(value) => serde_json::from_value::<Vec<ReasoningLevel>>(value.clone())
            .map_err(|_| protocol("模型推理能力格式无效"))?
            .into_iter()
            .map(|level| level.effort)
            .collect::<Vec<_>>(),
    };
    if levels.iter().any(|level| level.trim().is_empty()) {
        return Err(protocol("模型推理档位为空"));
    }
    let default = match model
        .get("default_reasoning_level")
        .filter(|value| !value.is_null())
    {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| protocol("模型默认推理档位无效"))?
                .to_owned(),
        ),
    };
    if !levels.is_empty()
        && default
            .as_ref()
            .is_some_and(|default| !levels.contains(default))
    {
        return Err(protocol("模型默认推理档位不在支持范围内"));
    }
    Ok((levels, default))
}

async fn acquire(client: &CredentialClient) -> Result<UsableCredentials, ModelFailure> {
    client
        .request()
        .map_err(ModelFailure::authentication)?
        .wait()
        .await
        .map_err(ModelFailure::authentication)
}
async fn recover(
    client: &CredentialClient,
    request: &mut AuthenticatedRequest,
    previous: &UsableCredentials,
    failure: &ModelFailure,
) -> Result<Option<UsableCredentials>, ModelFailure> {
    let recovery_failure = |reason: String| {
        let mut result = failure.clone();
        result.kind = ModelFailureKind::Authentication;
        result.message = format!("{}；认证恢复未完成：{reason}", failure.message);
        result
    };
    let Some(recovery) = client
        .recover(request, failure.status.unwrap_or_default())
        .map_err(|error| recovery_failure(error.to_string()))?
    else {
        return Ok(None);
    };
    tracing::debug!("模型请求收到认证拒绝，尝试一次同账号凭据恢复");
    let current = recovery
        .wait()
        .await
        .map_err(|error| recovery_failure(error.to_string()))?;
    same_account(previous, &current).map_err(|error| recovery_failure(error.to_string()))?;
    Ok(Some(current))
}
fn with_response(mut failure: ModelFailure, status: u16, id: Option<String>) -> ModelFailure {
    if failure.status.is_none() {
        failure.status = Some(status);
    }
    if failure.request_id.is_none() {
        failure.request_id = id;
    }
    failure
}
fn same_account(
    previous: &UsableCredentials,
    current: &UsableCredentials,
) -> Result<(), ModelFailure> {
    if previous.version().same_account(current.version()) {
        Ok(())
    } else {
        Err(ModelFailure::new(
            ModelFailureKind::Authentication,
            "账号身份已变化，原模型操作已停止",
        ))
    }
}
fn required_text<'a>(value: &'a Value, key: &str, message: &str) -> Result<&'a str, ModelFailure> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| protocol(message))
}
fn protocol(message: &str) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Protocol, message)
}
fn network(error: reqwest::Error) -> ModelFailure {
    ModelFailure::new(
        ModelFailureKind::Network,
        if error.is_timeout() {
            "模型连接超时或事件流中断"
        } else {
            "无法连接模型服务或事件流中断"
        },
    )
}
fn request_id(response: &Response) -> Option<String> {
    response
        .headers()
        .get("x-request-id")
        .and_then(|id| id.to_str().ok())
        .map(diagnostic)
}
fn diagnostic(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(MAX_DIAGNOSTIC_CHARS)
        .collect()
}
async fn read_bytes(mut response: Response) -> Result<Vec<u8>, ModelFailure> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_JSON_BYTES {
            return Err(protocol("模型服务响应超过允许大小"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn read_json(response: Response) -> Result<Value, ModelFailure> {
    serde_json::from_slice(&read_bytes(response).await?)
        .map_err(|_| protocol("模型服务返回无效 JSON"))
}
fn classify(status: Option<u16>, code: Option<&str>) -> ModelFailureKind {
    match (status, code) {
        (Some(401), _) => ModelFailureKind::Authentication,
        (Some(403), _) => ModelFailureKind::Permission,
        (Some(429), _)
        | (
            _,
            Some(
                "subscription_sharing_usage_limit_exceeded"
                | "subscription_sharing_usage_unavailable"
                | "rate_limit_exceeded"
                | "insufficient_quota",
            ),
        ) => ModelFailureKind::UsageLimit,
        _ => ModelFailureKind::Service,
    }
}
fn service_failure(
    value: &Value,
    status: Option<u16>,
    id: Option<String>,
    fallback: &str,
) -> ModelFailure {
    let error = value
        .get("error")
        .filter(|error| !error.is_null())
        .unwrap_or(value);
    let code = error.get("code").and_then(Value::as_str).map(diagnostic);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| value.get("detail").and_then(Value::as_str))
        .map(diagnostic)
        .or_else(|| {
            value
                .get("detail")
                .map(|detail| diagnostic(&detail.to_string()))
        })
        .unwrap_or_else(|| fallback.to_owned());
    ModelFailure {
        kind: classify(status, code.as_deref()),
        status,
        code,
        request_id: id.or_else(|| {
            value
                .get("request_id")
                .and_then(Value::as_str)
                .map(diagnostic)
        }),
        message,
    }
}
async fn http_failure(response: Response) -> ModelFailure {
    let status = Some(response.status().as_u16());
    let id = request_id(&response);
    match read_bytes(response).await {
        Ok(bytes) => {
            let value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(diagnostic(&String::from_utf8_lossy(&bytes))));
            let fallback = value
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .unwrap_or("模型服务拒绝请求");
            service_failure(&value, status, id, fallback)
        }
        Err(mut failure) => {
            failure.status = status;
            failure.request_id = id;
            failure
        }
    }
}

#[derive(Default)]
struct Sse {
    line: Vec<u8>,
    data: String,
    after_cr: bool,
    first_line: bool,
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

#[derive(Deserialize)]
struct CompletedResponse {
    status: String,
    output: Vec<OutputItem>,
}
#[derive(Deserialize)]
struct OutputItem {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Vec<Content>,
}
#[derive(Deserialize)]
struct Content {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}
async fn consume(
    mut response: Response,
    request: &mut AuthenticatedRequest,
) -> Result<String, ModelFailure> {
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
    let mut received = 0usize;
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        let mut failure = network(error);
        failure.request_id = id.clone();
        failure
    })? {
        received = received.saturating_add(chunk.len());
        if received > MAX_STREAM_BYTES {
            return Err(protocol("模型事件流超过允许大小"));
        }
        for byte in chunk {
            let Some(data) = parser.push(byte)? else {
                continue;
            };
            let event: Value =
                serde_json::from_str(&data).map_err(|_| protocol("模型事件不是有效 JSON"))?;
            let kind = required_text(&event, "type", "模型事件缺少类型")?;
            match kind {
                "response.completed" => {
                    let complete: CompletedResponse = serde_json::from_value(
                        event
                            .get("response")
                            .cloned()
                            .ok_or_else(|| protocol("完成事件缺少响应对象"))?,
                    )
                    .map_err(|_| protocol("完成响应对象结构无效"))?;
                    if complete.status != "completed" {
                        return Err(protocol("完成事件的响应状态不一致"));
                    }
                    let mut texts = Vec::new();
                    for item in complete.output {
                        if item.kind == "message" {
                            for content in item.content {
                                if content.kind == "output_text" {
                                    texts.push(
                                        content.text.ok_or_else(|| protocol("完成正文缺少文本"))?,
                                    );
                                }
                            }
                        }
                    }
                    return Ok(texts.join("\n\n"));
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
                "response.output_text.delta"
                | "response.output_item.added"
                | "response.content_part.added" => request.mark_output_started(),
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
