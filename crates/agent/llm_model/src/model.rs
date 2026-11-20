use agentelier_account::{
    AuthenticatedRequest, CredentialClient, ExecutionIdentity, UsableCredentials,
};
use agentelier_tools::ToolContract;
use reqwest::{Client, RequestBuilder, Response};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use std::{error::Error, fmt};

const CODEX_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";
pub(super) const CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const CODEX_CATALOG_VERSION: &str = "0.171.0";
const INPUT: &str = "请简短回复，确认连接正常";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_JSON_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIAGNOSTIC_CHARS: usize = 2048;

pub fn codex_tool_contract(slug: &str) -> ToolContract {
    match slug {
        "gpt-6-astra" => ToolContract::CodexWebSocket,
        _ => ToolContract::Flat,
    }
}

fn catalog_request(client: &Client, credentials: &UsableCredentials) -> RequestBuilder {
    authorize(
        client.get(format!(
            "{CODEX_MODELS_URL}?client_version={CODEX_CATALOG_VERSION}"
        )),
        credentials,
    )
}

pub(super) fn responses(client: &Client, credentials: &UsableCredentials) -> RequestBuilder {
    authorize(client.post(CODEX_RESPONSES_URL), credentials)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
}

pub(super) fn authorize(
    request: RequestBuilder,
    credentials: &UsableCredentials,
) -> RequestBuilder {
    request
        .bearer_auth(credentials.access_token())
        .header(
            "ChatGPT-Account-Id",
            &credentials.version().identity().workspace_id,
        )
        .header(
            reqwest::header::USER_AGENT,
            concat!("Agentelier/", env!("CARGO_PKG_VERSION")),
        )
}

#[derive(Clone)]
pub struct ModelInfo {
    pub slug: String,
    pub display_name: String,
    pub supported_reasoning_levels: Vec<String>,
    pub default_reasoning_level: Option<String>,
    pub image_input: Option<bool>,
    pub context_limits: crate::ContextLimits,
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
    pub usage: Option<Box<crate::ResponseUsage>>,
    pub response_id: Option<String>,
}
impl ModelFailure {
    pub(super) fn new(kind: ModelFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            status: None,
            code: None,
            request_id: None,
            message: message.into(),
            usage: None,
            response_id: None,
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
    execute_model(&credentials, operation, None).await
}

async fn execute_model(
    credentials: &CredentialClient,
    operation: ModelRequest,
    identity: Option<&ExecutionIdentity>,
) -> Result<ModelResult, ModelFailure> {
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "无法建立安全模型客户端"))?;
    let mut current = acquire(credentials).await?;
    let ModelRequest::Verify {
        slug,
        display_name,
        effort,
    } = operation
    else {
        let mut authentication = AuthenticatedRequest::new(&current);
        let catalog = loop {
            validate_identity(identity, &current)?;
            let response = catalog_request(&client, &current)
                .send()
                .await
                .map_err(network)?;
            if response.status().is_success() {
                let status = response.status().as_u16();
                let id = request_id(&response);
                let catalog = read_json(response)
                    .await
                    .map_err(|failure| with_response(failure, status, id))?;
                validate_identity(identity, &current)?;
                break catalog;
            }
            let failure = http_failure(response).await;
            if let Some(replacement) =
                recover(credentials, &mut authentication, &current, &failure).await?
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
        for model in models.iter().filter(|model| catalog_model(model)) {
            let slug = required_text(model, "slug", "可显示模型缺少 slug")?.to_owned();
            let display_name =
                required_text(model, "display_name", "可显示模型缺少 display_name")?.to_owned();
            let (supported_reasoning_levels, default_reasoning_level) =
                reasoning_capabilities(model)?;
            if result.iter().any(|entry: &ModelInfo| entry.slug == slug) {
                return Err(protocol("模型目录含有重复 slug"));
            }
            let image_input = model
                .get("input_modalities")
                .and_then(Value::as_array)
                .filter(|items| !items.is_empty() && items.iter().all(Value::is_string))
                .map(|items| items.iter().any(|item| item.as_str() == Some("image")));
            result.push(ModelInfo {
                image_input,
                context_limits: crate::ContextLimits::parse(model)?,
                slug,
                display_name,
                supported_reasoning_levels,
                default_reasoning_level,
            });
        }
        for model in &result {
            tracing::debug!(
                model = model.slug,
                context_window = model.context_limits.window,
                effective_window = model.context_limits.effective,
                auto_compact_limit = model.context_limits.automatic,
                "已读取模型上下文能力"
            );
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
    let output = request_text(&client, credentials, current, identity, payload).await?;
    Ok(ModelResult::Verified(ModelReply {
        slug,
        effort,
        model: display_name,
        text: text_output(output.messages),
    }))
}

pub struct BackgroundText {
    pub text: String,
    pub usage: Option<crate::ResponseUsage>,
    pub response_id: Option<String>,
}

pub async fn background_text(
    credentials: &CredentialClient,
    identity: &ExecutionIdentity,
    model: &ModelInfo,
    instruction: &str,
    input: &str,
    format: Value,
) -> Result<BackgroundText, ModelFailure> {
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "无法建立安全模型客户端"))?;
    let current = acquire(credentials).await?;
    let payload = serde_json::json!({
        "model": model.slug,
        "input": [{"role": "developer", "content": instruction}, {"role": "user", "content": input}],
        "store": false,
        "stream": true,
        "text": {"format": format},
    });
    let output = request_text(&client, credentials, current, Some(identity), payload).await?;
    Ok(BackgroundText {
        text: text_output(output.messages),
        usage: output.usage,
        response_id: output.response_id,
    })
}

fn text_output(messages: Vec<crate::OutputMessage>) -> String {
    messages
        .into_iter()
        .flat_map(|message| message.content.into_iter().map(|part| part.text))
        .collect::<Vec<_>>()
        .join("\n\n")
}

async fn request_text(
    client: &Client,
    credentials: &CredentialClient,
    mut current: UsableCredentials,
    identity: Option<&ExecutionIdentity>,
    mut payload: Value,
) -> Result<crate::GenerationResult, ModelFailure> {
    payload["instructions"] = Value::String(String::new());
    let body = serde_json::to_vec(&payload).map_err(|_| protocol("无法序列化模型验证请求"))?;
    let mut request = AuthenticatedRequest::new(&current);
    loop {
        validate_identity(identity, &current)?;
        let response = responses(client, &current)
            .body(body.clone())
            .send()
            .await
            .map_err(network)?;
        let outcome = if response.status().is_success() {
            let status = response.status().as_u16();
            let id = request_id(&response);
            crate::stream::consume(response, &mut request, None, None)
                .await
                .map_err(|failure| with_response(failure, status, id))
        } else {
            Err(http_failure(response).await)
        };
        match outcome {
            Ok(output) => {
                validate_identity(identity, &current)?;
                return Ok(output);
            }
            Err(failure) => {
                if let Some(replacement) =
                    recover(credentials, &mut request, &current, &failure).await?
                {
                    current = replacement;
                } else {
                    return Err(failure);
                }
            }
        }
    }
}

fn validate_identity(
    identity: Option<&ExecutionIdentity>,
    credentials: &UsableCredentials,
) -> Result<(), ModelFailure> {
    identity.map_or(Ok(()), |identity| {
        identity
            .validate(credentials)
            .map_err(ModelFailure::authentication)
    })
}

fn catalog_model(model: &Value) -> bool {
    model["visibility"] == "list"
        && model["input_modalities"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item == "text"))
        && model["allow_only_chatgpt_workspace_executor"].as_bool() != Some(true)
        && model["allow_only_codex_cloud_agent_orchestrator"].as_bool() != Some(true)
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

pub(super) async fn acquire(client: &CredentialClient) -> Result<UsableCredentials, ModelFailure> {
    client
        .request()
        .map_err(ModelFailure::authentication)?
        .wait()
        .await
        .map_err(ModelFailure::authentication)
}
pub(super) async fn recover(
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
pub(super) fn with_response(
    mut failure: ModelFailure,
    status: u16,
    id: Option<String>,
) -> ModelFailure {
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
    if previous.version().identity() == current.version().identity() {
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
pub(super) fn protocol(message: &str) -> ModelFailure {
    ModelFailure::new(ModelFailureKind::Protocol, message)
}
pub(super) fn network(error: reqwest::Error) -> ModelFailure {
    ModelFailure::new(
        ModelFailureKind::Network,
        if error.is_timeout() {
            "模型连接超时或事件流中断"
        } else {
            "无法连接模型服务或事件流中断"
        },
    )
}
pub(super) fn request_id(response: &Response) -> Option<String> {
    response
        .headers()
        .get("x-request-id")
        .and_then(|id| id.to_str().ok())
        .map(diagnostic)
}
pub(super) fn diagnostic(value: &str) -> String {
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
                | "usage_limit_reached"
                | "rate_limit_exceeded"
                | "insufficient_quota",
            ),
        ) => ModelFailureKind::UsageLimit,
        _ => ModelFailureKind::Service,
    }
}
pub(super) fn service_failure(
    value: &Value,
    status: Option<u16>,
    id: Option<String>,
    fallback: &str,
) -> ModelFailure {
    let error = value
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| value.get("detail").filter(|detail| detail.is_object()))
        .unwrap_or(value);
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| error.get("type").and_then(Value::as_str))
        .map(diagnostic);
    let mut message = error
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
    let usage = match crate::ResponseUsage::parse(value) {
        Ok(usage) => usage,
        Err(failure) => {
            message.push_str(&format!("；响应 usage 无效：{failure}"));
            None
        }
    };
    ModelFailure {
        usage: usage.map(Box::new),
        response_id: value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned),
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
pub(super) async fn http_failure(response: Response) -> ModelFailure {
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

pub async fn task_model(
    credentials: CredentialClient,
    identity: &agentelier_account::ExecutionIdentity,
    slug: &str,
) -> Result<ModelInfo, ModelFailure> {
    task_catalog(&credentials, identity)
        .await?
        .into_iter()
        .find(|model| model.slug == slug)
        .ok_or_else(|| {
            ModelFailure::new(
                ModelFailureKind::Permission,
                format!("当前账号模型目录不包含会话固定模型：{slug}"),
            )
        })
}

pub async fn task_catalog(
    credentials: &CredentialClient,
    identity: &ExecutionIdentity,
) -> Result<Vec<ModelInfo>, ModelFailure> {
    match execute_model(credentials, ModelRequest::Catalog, Some(identity)).await? {
        ModelResult::Catalog(models) => Ok(models),
        _ => Err(protocol("模型能力请求返回了非目录结果")),
    }
}
