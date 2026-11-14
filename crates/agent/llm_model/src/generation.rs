use crate::model::{
    ModelFailure, ModelFailureKind, acquire, http_failure, network, recover, request_id,
    with_response,
};
use crate::protocol::ProtocolHistory;
use crate::stream::{GenerationResult, STREAM_IDLE_TIMEOUT, StreamEvent, consume};
use agentelier_account::{AuthenticatedRequest, CredentialClient};
use reqwest::Client;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct GenerationRequest {
    pub slug: String,
    pub effort: Option<String>,
    pub context: ProtocolHistory,
    pub account_subject: String,
}

pub async fn generate(
    credentials: CredentialClient,
    generation: GenerationRequest,
    progress: Sender<StreamEvent>,
) -> Result<GenerationResult, ModelFailure> {
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STREAM_IDLE_TIMEOUT)
        .build()
        .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "无法建立安全模型客户端"))?;
    let mut current = acquire(&credentials).await?;
    if current.version().subject() != generation.account_subject {
        return Err(ModelFailure::new(
            ModelFailureKind::Authentication,
            "账号身份已变化，请创建新聊天会话",
        ));
    }
    let mut payload = serde_json::json!({
        "model": generation.slug,
        "input": generation.context.into_input(),
        "include": ["reasoning.encrypted_content"],
        "store": false,
        "stream": true,
    });
    if let Some(effort) = &generation.effort {
        payload["reasoning"] = serde_json::json!({"effort": effort});
    }
    tracing::debug!(
        model = generation.slug,
        effort = generation.effort,
        "开始流式模型请求"
    );
    let mut request = AuthenticatedRequest::new(&current);
    loop {
        let response = tokio::time::timeout(
            STREAM_IDLE_TIMEOUT,
            client
                .post(RESPONSES_URL)
                .bearer_auth(current.access_token())
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .json(&payload)
                .send(),
        )
        .await
        .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "等待模型响应超时"))?
        .map_err(network)?;
        let outcome = if response.status().is_success() {
            let status = response.status().as_u16();
            let id = request_id(&response);
            consume(response, &mut request, Some(&progress), true)
                .await
                .map_err(|failure| with_response(failure, status, id))
        } else {
            Err(http_failure(response).await)
        };
        match outcome {
            Ok(result) => {
                if !result.output.is_replayable() {
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
