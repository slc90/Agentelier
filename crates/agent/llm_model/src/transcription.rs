use crate::model::{acquire, authorize, http_failure, request_id};
use crate::{ModelFailure, ModelFailureKind};
use agentelier_account::{CredentialClient, ExecutionIdentity};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use std::time::Duration;

const TRANSCRIPTION_URL: &str = "https://chatgpt.com/backend-api/transcribe";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const TRANSCRIPTION_TIMEOUT: Duration = Duration::from_secs(180);
pub const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
struct TranscriptResponse {
    text: String,
}

pub async fn transcribe_audio(
    credentials: &CredentialClient,
    identity: &ExecutionIdentity,
    mime: &'static str,
    bytes: Vec<u8>,
) -> Result<String, ModelFailure> {
    tokio::select! { biased;
        _ = identity.invalidated() => Err(ModelFailure::new(ModelFailureKind::Authentication, "转写所绑定的 Codex 授权已失效")),
        result = request(credentials, identity, mime, bytes) => result,
    }
}

async fn request(
    credentials: &CredentialClient,
    identity: &ExecutionIdentity,
    mime: &'static str,
    bytes: Vec<u8>,
) -> Result<String, ModelFailure> {
    let filename = match mime {
        "audio/wav" => "audio.wav",
        "audio/mpeg" => "audio.mp3",
        _ => {
            return Err(ModelFailure::new(
                ModelFailureKind::Protocol,
                "转写音频格式无效",
            ));
        }
    };
    let credentials = acquire(credentials).await?;
    identity.validate(&credentials).map_err(|failure| {
        ModelFailure::new(ModelFailureKind::Authentication, failure.to_string())
    })?;
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TRANSCRIPTION_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(network_failure)?;
    let file = Part::bytes(bytes)
        .file_name(filename)
        .mime_str(mime)
        .map_err(network_failure)?;
    let mut response = authorize(client.post(TRANSCRIPTION_URL), &credentials)
        .multipart(Form::new().part("file", file))
        .send()
        .await
        .map_err(network_failure)?;
    if !response.status().is_success() {
        return Err(http_failure(response).await);
    }
    let id = request_id(&response);
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_failure)? {
        if chunk.len() > MAX_TRANSCRIPT_BYTES.saturating_sub(bytes.len()) {
            return Err(ModelFailure::new(
                ModelFailureKind::Protocol,
                "转写响应超过 64 KiB",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let transcript: TranscriptResponse = serde_json::from_slice(&bytes).map_err(|_| {
        ModelFailure::new(ModelFailureKind::Incomplete, "转写服务未返回完整 JSON 文稿")
    })?;
    if transcript.text.trim().is_empty() {
        return Err(ModelFailure::new(
            ModelFailureKind::Incomplete,
            "转写服务没有识别到文字",
        ));
    }
    tracing::debug!(
        request_id = id.as_deref(),
        bytes = transcript.text.len(),
        "Codex 音频转写已返回完整文稿"
    );
    Ok(transcript.text)
}

fn network_failure(failure: reqwest::Error) -> ModelFailure {
    let message = if failure.is_timeout() {
        "Codex 转写请求超时"
    } else {
        "Codex 转写网络请求失败"
    };
    ModelFailure::new(ModelFailureKind::Network, message)
}
