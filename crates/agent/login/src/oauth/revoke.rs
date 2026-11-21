use super::{CLIENT_ID, CodexOAuthCredentials, token_client};
use crate::codex::ISSUER;
use reqwest::StatusCode;
use std::{fmt, time::Duration};

const REVOKE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum RevocationFailure {
    Network,
    Timeout,
    Rejected(StatusCode),
}

impl fmt::Display for RevocationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network => f.write_str("无法连接 OAuth 撤销服务"),
            Self::Timeout => f.write_str("等待 OAuth 撤销服务超时"),
            Self::Rejected(status) => write!(f, "OAuth 服务拒绝撤销请求（HTTP {status}）"),
        }
    }
}

impl std::error::Error for RevocationFailure {}

impl CodexOAuthCredentials {
    pub async fn revoke(&self) -> Result<(), RevocationFailure> {
        let response = token_client()
            .map_err(|_| RevocationFailure::Network)?
            .post(format!("{ISSUER}/oauth/revoke"))
            .timeout(REVOKE_TIMEOUT)
            .json(&serde_json::json!({
                "token": self.tokens.refresh_token.as_str(),
                "token_type_hint": "refresh_token",
                "client_id": CLIENT_ID,
            }))
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    RevocationFailure::Timeout
                } else {
                    RevocationFailure::Network
                }
            })?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(RevocationFailure::Rejected(status))
        }
    }
}
