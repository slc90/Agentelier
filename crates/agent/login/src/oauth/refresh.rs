use super::{CLIENT_ID, CodexOAuthCredentials, OAuthTokens, TokenResponse, token_client};
use crate::CodexReadFailure;
use crate::codex::{ISSUER, unix_time};
use reqwest::StatusCode;
use std::fmt;

#[derive(Debug)]
pub enum RefreshFailure {
    Reauthorize,
    ChangedIdentity,
    Rejected(StatusCode),
    Network,
    Timeout,
    InvalidResponse,
    Credentials(CodexReadFailure),
}

impl RefreshFailure {
    pub fn needs_login(&self) -> bool {
        matches!(self, Self::Reauthorize | Self::ChangedIdentity)
    }
}

impl fmt::Display for RefreshFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reauthorize => f.write_str("Codex OAuth 授权已失效，请重新登录 ChatGPT"),
            Self::ChangedIdentity => f.write_str("续期返回了不同的账号或工作区，请重新登录"),
            Self::Rejected(status) => write!(f, "OAuth 服务拒绝凭据续期（HTTP {status}）"),
            Self::Network => f.write_str("无法连接 OAuth 服务以续期"),
            Self::Timeout => f.write_str("等待 OAuth 凭据续期超时"),
            Self::InvalidResponse => f.write_str("OAuth 凭据续期响应无效"),
            Self::Credentials(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RefreshFailure {}

impl CodexOAuthCredentials {
    pub async fn refresh(&self) -> Result<Self, RefreshFailure> {
        let response = token_client()
            .map_err(|_| RefreshFailure::Network)?
            .post(format!("{ISSUER}/oauth/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", self.tokens.refresh_token.as_str()),
            ])
            .send()
            .await
            .map_err(network_failure)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.json::<serde_json::Value>().await;
            return Err(match body {
                Ok(body) if permanently_rejected(&body) => RefreshFailure::Reauthorize,
                _ => RefreshFailure::Rejected(status),
            });
        }
        let response: TokenResponse = response.json().await.map_err(network_failure)?;
        let expires_at = match response.expires_in {
            Some(seconds) => unix_time()
                .map_err(RefreshFailure::Credentials)?
                .saturating_add(seconds),
            None if response.access_token.is_some() => u64::MAX,
            None => self.tokens.expires_at,
        };
        let previous = &self.tokens;
        let replacement = Self::from_tokens(OAuthTokens {
            id_token: response
                .id_token
                .unwrap_or_else(|| previous.id_token.clone()),
            access_token: response
                .access_token
                .unwrap_or_else(|| previous.access_token.clone()),
            refresh_token: response
                .refresh_token
                .unwrap_or_else(|| previous.refresh_token.clone()),
            expires_at,
        })
        .map_err(|failure| match failure {
            CodexReadFailure::IdentityMismatch => RefreshFailure::ChangedIdentity,
            other => RefreshFailure::Credentials(other),
        })?;
        if replacement.summary.identity != self.summary.identity {
            return Err(RefreshFailure::ChangedIdentity);
        }
        Ok(replacement)
    }
}

fn network_failure(error: reqwest::Error) -> RefreshFailure {
    if error.is_timeout() {
        RefreshFailure::Timeout
    } else if error.is_decode() {
        RefreshFailure::InvalidResponse
    } else {
        RefreshFailure::Network
    }
}

fn permanently_rejected(body: &serde_json::Value) -> bool {
    let error = &body["error"];
    let code = error.as_str().or_else(|| error["code"].as_str());
    matches!(
        code,
        Some(
            "invalid_grant"
                | "refresh_token_expired"
                | "refresh_token_reused"
                | "refresh_token_invalidated"
                | "refresh_token_revoked"
                | "invalid_refresh_token"
        )
    )
}
