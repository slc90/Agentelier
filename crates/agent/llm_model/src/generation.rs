use crate::compaction::{RequestPurpose, StreamResult};
use crate::model::{
    ModelFailure, ModelFailureKind, acquire, http_failure, network, recover, request_id,
    with_response,
};
use crate::protocol::ProtocolHistory;
use crate::stream::{GenerationResult, STREAM_IDLE_TIMEOUT, StreamEvent, consume_for};
use agentelier_account::{AuthenticatedRequest, CredentialClient, ExecutionIdentity};
use agentelier_prompts::InstructionSnapshot;
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

struct RequestBytes(Vec<u8>);
impl std::io::Write for RequestBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_REQUEST_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other(
                "完整模型请求超过 32 MiB，历史保持不变",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct GenerationRequest {
    pub slug: String,
    pub effort: Option<String>,
    pub context: ProtocolHistory,
    pub instructions: Arc<InstructionSnapshot>,
    pub tools: Arc<agentelier_tools::ToolCatalog>,
}

pub async fn generate(
    credentials: CredentialClient,
    generation: GenerationRequest,
    identity: &ExecutionIdentity,
    progress: Sender<StreamEvent>,
) -> Result<GenerationResult, ModelFailure> {
    match execute_request(
        credentials,
        generation,
        identity,
        progress,
        RequestPurpose::Generation,
    )
    .await?
    {
        StreamResult::Generation(result) => Ok(result),
        StreamResult::Compaction(_) => Err(crate::model::protocol("普通请求返回了压缩结果")),
    }
}
pub async fn compact(
    credentials: CredentialClient,
    generation: GenerationRequest,
    identity: &ExecutionIdentity,
    progress: Sender<StreamEvent>,
) -> Result<crate::CompactionResult, ModelFailure> {
    match execute_request(
        credentials,
        generation,
        identity,
        progress,
        RequestPurpose::Compaction,
    )
    .await?
    {
        StreamResult::Compaction(result) => Ok(result),
        StreamResult::Generation(_) => Err(crate::model::protocol("压缩请求返回了普通结果")),
    }
}
async fn execute_request(
    credentials: CredentialClient,
    generation: GenerationRequest,
    identity: &ExecutionIdentity,
    progress: Sender<StreamEvent>,
    purpose: RequestPurpose,
) -> Result<StreamResult, ModelFailure> {
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STREAM_IDLE_TIMEOUT)
        .build()
        .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "无法建立安全模型客户端"))?;
    let mut current = acquire(&credentials).await?;
    identity.validate(&current).map_err(|failure| {
        ModelFailure::new(ModelFailureKind::Authentication, failure.to_string())
    })?;
    generation
        .instructions
        .validate()
        .map_err(|_| ModelFailure::new(ModelFailureKind::Protocol, "会话指令快照无效"))?;
    let mut input: Vec<_> = generation
        .instructions
        .messages()
        .into_iter()
        .map(|message| serde_json::json!({"role": message.role, "content": message.text}))
        .collect();
    input.extend(generation.context.into_input());
    if purpose == RequestPurpose::Compaction {
        input.push(serde_json::json!({"type":"compaction_trigger"}));
    }
    let mut payload = serde_json::json!({
        "model": generation.slug,
        "input": input,
        "include": ["reasoning.encrypted_content"],
        "store": false,
        "stream": true,
    });
    payload["tools"] = serde_json::Value::Array(generation.tools.definitions().iter().map(|tool| serde_json::json!({
        "type":"function", "name":tool.name, "description":tool.description, "parameters":tool.parameters, "strict":true
    })).collect());
    payload["tool_choice"] = serde_json::json!("auto");
    payload["parallel_tool_calls"] = serde_json::json!(true);
    if let Some(effort) = &generation.effort {
        payload["reasoning"] = serde_json::json!({"effort": effort});
    }
    let mut body = RequestBytes(Vec::new());
    serde_json::to_writer(&mut body, &payload).map_err(|error| {
        ModelFailure::new(
            ModelFailureKind::Protocol,
            format!("模型请求大小或序列化失败：{error}"),
        )
    })?;
    for item in payload["input"].as_array().into_iter().flatten() {
        if let Some(content) = item.get("output").and_then(serde_json::Value::as_array) {
            for image in content.iter().filter(|part| part["type"] == "input_image") {
                tracing::debug!(
                    call_id = item["call_id"].as_str(),
                    output_kind = "content_array",
                    item_type = image["type"].as_str(),
                    detail = image["detail"].as_str(),
                    data_url = image["image_url"]
                        .as_str()
                        .is_some_and(|url| url.starts_with("data:image/")),
                    "请求中已包含真实图片内容项"
                );
            }
        }
    }
    tracing::debug!(
        purpose = if purpose == RequestPurpose::Compaction {
            "compaction"
        } else {
            "generation"
        },
        model = generation.slug,
        effort = generation.effort,
        request_bytes = body.0.len(),
        "开始流式模型请求"
    );
    let mut request = AuthenticatedRequest::new(&current);
    loop {
        identity.validate(&current).map_err(|failure| {
            ModelFailure::new(ModelFailureKind::Authentication, failure.to_string())
        })?;
        let response = tokio::time::timeout(
            STREAM_IDLE_TIMEOUT,
            client
                .post(RESPONSES_URL)
                .bearer_auth(current.access_token())
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.0.clone())
                .send(),
        )
        .await
        .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "等待模型响应超时"))?
        .map_err(network)?;
        let outcome = if response.status().is_success() {
            let status = response.status().as_u16();
            let id = request_id(&response);
            consume_for(response, &mut request, Some(&progress), true, purpose)
                .await
                .map_err(|failure| with_response(failure, status, id))
        } else {
            let failure = http_failure(response).await;
            if let Some(usage) = failure.usage.clone() {
                progress
                    .send(StreamEvent::Usage {
                        response_id: failure.response_id.clone(),
                        usage: *usage,
                    })
                    .await
                    .map_err(|_| crate::model::protocol("模型用量接收端已关闭"))?;
            }
            Err(failure)
        };
        match outcome {
            Ok(result) => {
                if matches!(&result, StreamResult::Generation(result) if !result.output.is_replayable())
                {
                    return Err(ModelFailure::new(
                        ModelFailureKind::Protocol,
                        "完成响应缺少无状态续接所需的加密推理项",
                    ));
                }
                return Ok(result);
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
