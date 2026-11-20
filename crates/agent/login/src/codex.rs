use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_AUTH_BYTES: u64 = 64 * 1024;
const ISSUER: &str = "https://auth.openai.com";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodexAccountSummary {
    pub identity: CodexIdentity,
    pub email: Option<String>,
    pub expired: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CodexIdentity {
    pub subject: String,
    pub workspace_id: String,
}

pub struct CodexCredentials {
    pub summary: CodexAccountSummary,
    pub(crate) access_token: String,
    pub(crate) expires_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexReadFailure {
    Unreadable,
    Unsupported,
    MissingTokens,
    InvalidTokens,
    IdentityMismatch,
    Clock,
}

impl fmt::Display for CodexReadFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unreadable => "无法读取 Codex 凭据文件",
            Self::Unsupported => "不支持此文件格式或认证方式，请选择 Codex ChatGPT OAuth 凭据",
            Self::MissingTokens => "Codex OAuth 凭据缺少必要令牌",
            Self::InvalidTokens => "Codex OAuth 令牌格式或声明无效",
            Self::IdentityMismatch => "Codex OAuth 凭据的账号或工作区标识不一致",
            Self::Clock => "无法判断 Codex OAuth 凭据是否过期",
        })
    }
}

#[derive(Deserialize)]
struct AuthFile {
    auth_mode: Option<String>,
    #[serde(rename = "OPENAI_API_KEY")]
    api_key: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    account_id: Option<String>,
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

pub fn read_codex_credentials(path: &Path) -> Result<CodexCredentials, CodexReadFailure> {
    if !path.is_absolute() || !path.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(CodexReadFailure::Unreadable);
    }
    let file = File::open(path).map_err(|_| CodexReadFailure::Unreadable)?;
    let mut bytes = Vec::new();
    file.take(MAX_AUTH_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| CodexReadFailure::Unreadable)?;
    if bytes.len() as u64 > MAX_AUTH_BYTES {
        return Err(CodexReadFailure::Unsupported);
    }
    let file: AuthFile =
        serde_json::from_slice(&bytes).map_err(|_| CodexReadFailure::Unsupported)?;
    if !matches!(file.auth_mode.as_deref(), Some("chatgpt") | None)
        || file.api_key.is_some_and(|key| !key.is_empty())
    {
        return Err(CodexReadFailure::Unsupported);
    }
    let tokens = file.tokens.ok_or(match file.auth_mode {
        Some(_) => CodexReadFailure::MissingTokens,
        None => CodexReadFailure::Unsupported,
    })?;
    let id = claims(required_token(tokens.id_token.as_deref())?)?;
    let access_token = tokens.access_token.ok_or(CodexReadFailure::MissingTokens)?;
    let access = claims(required_token(Some(&access_token))?)?;
    required_token(tokens.refresh_token.as_deref())?;
    let workspace_id = &access.auth.chatgpt_account_id;
    if id.sub != access.sub
        || &id.auth.chatgpt_account_id != workspace_id
        || tokens
            .account_id
            .as_ref()
            .is_some_and(|account| account != workspace_id)
        || matches!((id.auth.user(), access.auth.user()), (Some(a), Some(b)) if a != b)
    {
        return Err(CodexReadFailure::IdentityMismatch);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CodexReadFailure::Clock)?
        .as_secs();
    Ok(CodexCredentials {
        summary: CodexAccountSummary {
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
        access_token,
        expires_at: access.exp,
    })
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
    if signature.is_empty()
        || URL_SAFE_NO_PAD.decode(signature).is_err()
        || header.is_empty()
        || !jsonwebtoken::decode_header(token)
            .is_ok_and(|header| header.alg == jsonwebtoken::Algorithm::RS256)
    {
        return Err(CodexReadFailure::InvalidTokens);
    }
    // Imported claims describe the local file. This path has no signing keys or remote session check.
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
