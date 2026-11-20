use crate::model::{acquire, authorize, http_failure, request_id};
use agentelier_account::{CredentialClient, ExecutionIdentity, UsableCredentials};
use agentelier_tools::{
    GENERATED_IMAGE_MODEL, MAX_GENERATION_PROMPT_CHARS, MAX_ORIGINAL_IMAGE_BYTES,
    MAX_TOOL_BODY_BYTES, OriginalImage, ToolPayload, ToolResult, ToolStatus,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::{Client, Response};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

const CODEX_IMAGES_URL: &str = "https://chatgpt.com/backend-api/codex/images/generations";
const MAX_IMAGE_BASE64_BYTES: usize = MAX_ORIGINAL_IMAGE_BYTES.div_ceil(3) * 4;
const MAX_RESPONSE_BYTES: usize = MAX_IMAGE_BASE64_BYTES + 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    prompt: String,
}

#[derive(Deserialize)]
struct GenerationResponse {
    data: [Generation; 1],
}

#[derive(Deserialize)]
struct Generation {
    b64_json: String,
}

pub async fn generate_image(
    credentials: &CredentialClient,
    identity: &ExecutionIdentity,
    arguments: &str,
    cancel: &CancellationToken,
) -> ToolResult {
    let Some(args) = (arguments.len() <= MAX_TOOL_BODY_BYTES)
        .then(|| serde_json::from_str::<Arguments>(arguments).ok())
        .flatten()
        .filter(|args| {
            !args.prompt.trim().is_empty()
                && args.prompt.chars().count() <= MAX_GENERATION_PROMPT_CHARS
        })
    else {
        return failed("image_arguments", "需要一条非空图片提示词，最多 8000 字符");
    };
    generate(credentials, identity, args, cancel)
        .await
        .unwrap_or_else(|failure| failure)
}

async fn generate(
    credentials: &CredentialClient,
    identity: &ExecutionIdentity,
    args: Arguments,
    cancel: &CancellationToken,
) -> Result<ToolResult, ToolResult> {
    let (bytes, id) = tokio::select! { biased;
        _ = cancel.cancelled() => return Err(cancelled()),
        _ = identity.invalidated() => return Err(cancelled()),
        result = download(credentials, identity, &args.prompt) => result?,
    };
    let image = tokio::task::spawn_blocking(move || decode_response(&bytes))
        .await
        .map_err(|_| failed("image_decode", "图像解码任务异常结束"))?;
    if cancel.is_cancelled() || identity.is_invalidated() {
        return Err(cancelled());
    }
    let image = image?;
    tracing::debug!(
        model = GENERATED_IMAGE_MODEL,
        request_id = id.as_deref(),
        encoded_bytes = image.descriptor().encoded_bytes,
        "Codex 图片生成已完成并解码"
    );
    Ok(ToolResult {
        status: ToolStatus::Succeeded,
        output: ToolPayload::GeneratedImage {
            prompt: args.prompt,
            image: Arc::new(image),
        },
        cleanup_failed: false,
    })
}

async fn download(
    credentials: &CredentialClient,
    identity: &ExecutionIdentity,
    prompt: &str,
) -> Result<(Vec<u8>, Option<String>), ToolResult> {
    let credentials = acquire(credentials).await.map_err(|failure| {
        ToolResult::not_executed("image_authentication", &failure.to_string())
    })?;
    identity.validate(&credentials).map_err(|failure| {
        ToolResult::not_executed("image_authentication", &failure.to_string())
    })?;
    let response = request(&credentials, prompt).await?;
    let id = request_id(&response);
    let bytes = read_response(response).await?;
    Ok((bytes, id))
}

async fn request(credentials: &UsableCredentials, prompt: &str) -> Result<Response, ToolResult> {
    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(network_failure)?;
    let response = authorize(client.post(CODEX_IMAGES_URL), credentials)
        .json(&serde_json::json!({
            "model": GENERATED_IMAGE_MODEL,
            "prompt": prompt,
            "quality": "auto",
            "size": "auto"
        }))
        .send()
        .await
        .map_err(network_failure)?;
    if response.status().is_success() {
        return Ok(response);
    }
    let failure = http_failure(response).await;
    let code = match failure.kind {
        crate::ModelFailureKind::Authentication => "image_authentication",
        crate::ModelFailureKind::Permission => "image_permission",
        crate::ModelFailureKind::UsageLimit => "image_usage_limit",
        _ => "image_service",
    };
    Err(failed(code, &failure.to_string()))
}

async fn read_response(mut response: Response) -> Result<Vec<u8>, ToolResult> {
    let limit = || failed("image_response_limit", "图片服务返回超过大小限额");
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(limit());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_failure)? {
        if chunk.len() > MAX_RESPONSE_BYTES.saturating_sub(bytes.len()) {
            return Err(limit());
        }
        bytes.extend(chunk);
    }
    Ok(bytes)
}

fn decode_response(bytes: &[u8]) -> Result<OriginalImage, ToolResult> {
    let GenerationResponse { data: [result] } = serde_json::from_slice(bytes)
        .map_err(|_| failed("image_incomplete", "图片服务没有返回一项完整的原图"))?;
    if result.b64_json.len() > MAX_IMAGE_BASE64_BYTES {
        return Err(failed("image_response_limit", "图片原图超过大小限额"));
    }
    let bytes = STANDARD
        .decode(result.b64_json)
        .map_err(|_| failed("image_encoding", "图片服务返回无效的 Base64 编码"))?;
    OriginalImage::decode(bytes).map_err(|error| failed("image_decode", &error.to_string()))
}

fn network_failure(error: reqwest::Error) -> ToolResult {
    if error.is_timeout() {
        ToolResult::error(
            ToolStatus::TimedOut,
            "image_timeout",
            "图片生成连接或响应超时",
        )
    } else {
        failed("image_interrupted", "无法连接图片服务或响应已中断")
    }
}

fn failed(code: &str, message: &str) -> ToolResult {
    ToolResult::error(ToolStatus::Failed, code, message)
}

fn cancelled() -> ToolResult {
    ToolResult::error(
        ToolStatus::Cancelled,
        "image_cancelled",
        "图片生成已停止，未保存产物",
    )
}
