use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const ISSUER: &str = "https://auth.openai.com";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct CodexAccountSummary {
    pub identity: CodexIdentity,
    pub email: Option<String>,
    #[serde(skip)]
    pub expired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CodexIdentity {
    pub subject: String,
    pub workspace_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexReadFailure {
    MissingTokens,
    InvalidTokens,
    IdentityMismatch,
    Clock,
}

impl fmt::Display for CodexReadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MissingTokens => "Codex OAuth 凭据缺少必要令牌",
            Self::InvalidTokens => "Codex OAuth 令牌格式或声明无效",
            Self::IdentityMismatch => "Codex OAuth 凭据的账号或工作区标识不一致",
            Self::Clock => "无法判断 Codex OAuth 凭据是否过期",
        })
    }
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    exp: u64,
    email: Option<String>,
    #[serde(rename = "https://api.openai.com/auth")]
    auth: ChatGptClaims,
}

#[derive(Deserialize)]
struct ChatGptClaims {
    chatgpt_account_id: String,
    chatgpt_user_id: Option<String>,
    user_id: Option<String>,
}

pub(crate) fn summarize_tokens(
    id_token: &str,
    access_token: &str,
    refresh_token: &str,
) -> Result<(CodexAccountSummary, u64), CodexReadFailure> {
    let id = claims(required_token(Some(id_token))?)?;
    let access = claims(required_token(Some(access_token))?)?;
    required_token(Some(refresh_token))?;
    let workspace_id = &access.auth.chatgpt_account_id;
    if id.sub != access.sub
        || &id.auth.chatgpt_account_id != workspace_id
        || matches!((id.auth.user(), access.auth.user()), (Some(a), Some(b)) if a != b)
    {
        return Err(CodexReadFailure::IdentityMismatch);
    }
    let now = unix_time()?;
    Ok((
        CodexAccountSummary {
            identity: CodexIdentity {
                subject: id.sub,
                workspace_id: access.auth.chatgpt_account_id,
            },
            email: id.email.filter(|email| {
                email.len() <= 254
                    && email.contains('@')
                    && !email.chars().any(char::is_whitespace)
                    && !email.chars().any(char::is_control)
            }),
            expired: access.exp <= now,
        },
        access.exp,
    ))
}

pub(crate) fn unix_time() -> Result<u64, CodexReadFailure> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| CodexReadFailure::Clock)
}

impl ChatGptClaims {
    fn user(&self) -> Option<&str> {
        self.chatgpt_user_id.as_deref().or(self.user_id.as_deref())
    }
}

fn required_token(token: Option<&str>) -> Result<&str, CodexReadFailure> {
    token
        .filter(|token| !token.is_empty())
        .ok_or(CodexReadFailure::MissingTokens)
}

fn claims(token: &str) -> Result<Claims, CodexReadFailure> {
    let [header, payload, signature]: [&str; 3] = token
        .splitn(4, '.')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| CodexReadFailure::InvalidTokens)?;
    if signature.is_empty() || URL_SAFE_NO_PAD.decode(signature).is_err() {
        return Err(CodexReadFailure::InvalidTokens);
    }
    let header = URL_SAFE_NO_PAD
        .decode(header)
        .map_err(|_| CodexReadFailure::InvalidTokens)?;
    let header: serde_json::Value =
        serde_json::from_slice(&header).map_err(|_| CodexReadFailure::InvalidTokens)?;
    if header["alg"] != "RS256" {
        return Err(CodexReadFailure::InvalidTokens);
    }
    // These claims are used for identity consistency. The API verifies bearer authorization.
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| CodexReadFailure::InvalidTokens)?;
    let claims: Claims =
        serde_json::from_slice(&payload).map_err(|_| CodexReadFailure::InvalidTokens)?;
    if claims.iss != ISSUER || !valid_id(&claims.sub) || !valid_id(&claims.auth.chatgpt_account_id)
    {
        return Err(CodexReadFailure::InvalidTokens);
    }
    Ok(claims)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'|' | b'.' | b':')
        })
}
