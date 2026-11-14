mod callback;
pub(crate) mod identity;

use crate::authentication::{AccountRecord, ISSUER, Registration, Session};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{error::Error, fmt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use url::Url;

const AUTHORIZE_ENDPOINT: &str = "https://auth.openai.com/api/accounts/authorize";
pub(crate) const TOKEN_ENDPOINT: &str = "https://auth.openai.com/api/accounts/oauth/token";
pub(crate) const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const RANDOM_BYTES: usize = 32;
pub(crate) const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub struct AuthorizationContext {
    pub(crate) host_id: String,
    pub(crate) client_id: Option<String>,
    pub(crate) subject: Option<String>,
    pub(crate) email: Option<String>,
}

pub struct VerifiedLogin {
    pub(crate) record: AccountRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginStage {
    Prepare,
    Callback,
    Token,
    Identity,
    Save,
    Cancelled,
    Timeout,
}

#[derive(Debug)]
pub struct LoginFailure {
    pub stage: LoginStage,
    message: String,
}

impl LoginFailure {
    pub(crate) fn new(stage: LoginStage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
    pub(crate) fn storage(stage: LoginStage, error: impl fmt::Display) -> Self {
        Self::new(stage, error.to_string())
    }
    pub fn cancelled() -> Self {
        Self::new(LoginStage::Cancelled, "登录已取消")
    }
}
impl fmt::Display for LoginFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stage = match self.stage {
            LoginStage::Prepare => "准备登录",
            LoginStage::Callback => "浏览器回调",
            LoginStage::Token => "交换授权码",
            LoginStage::Identity => "校验账号身份",
            LoginStage::Save => "保存登录凭据",
            LoginStage::Cancelled => "取消登录",
            LoginStage::Timeout => "等待授权",
        };
        write!(f, "{stage}：{}", self.message)
    }
}
impl Error for LoginFailure {}

pub struct BrowserLogin {
    context: AuthorizationContext,
    listener: TcpListener,
    client: reqwest::Client,
    authorization_url: String,
    redirect_uri: String,
    state: String,
    nonce: String,
    verifier: String,
    issued_client: Option<String>,
    code: Option<String>,
}

impl BrowserLogin {
    pub async fn start(context: AuthorizationContext, consent: bool) -> Result<Self, LoginFailure> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|error| LoginFailure::storage(LoginStage::Callback, error))?;
        let port = listener
            .local_addr()
            .map_err(|error| LoginFailure::storage(LoginStage::Callback, error))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
        let state = random()?;
        let nonce = random()?;
        let verifier = random()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = Url::parse(AUTHORIZE_ENDPOINT)
            .map_err(|_| LoginFailure::new(LoginStage::Prepare, "授权地址无效"))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair(
                    "client_id",
                    context.client_id.as_deref().unwrap_or(DYNAMIC_CLIENT),
                )
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", &redirect_uri)
                .append_pair("scope", SCOPES)
                .append_pair("resource", RESOURCE)
                .append_pair("state", &state)
                .append_pair("nonce", &nonce)
                .append_pair("code_challenge_method", "S256")
                .append_pair("code_challenge", &challenge)
                .append_pair("ext_agent_host_id", &context.host_id);
            if context.client_id.is_none() {
                query.append_pair("agent_name_hint", "Agentelier");
            }
            if let Some(email) = &context.email {
                query.append_pair("login_hint", email);
            }
            if consent {
                query.append_pair("prompt", "consent");
            }
        }
        let client = http_client()?;
        Ok(Self {
            context,
            listener,
            client,
            authorization_url: url.into(),
            redirect_uri,
            state,
            nonce,
            verifier,
            issued_client: None,
            code: None,
        })
    }

    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }

    pub async fn wait_for_callback(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<String, LoginFailure> {
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(LoginFailure::cancelled()),
            result = tokio::time::timeout(LOGIN_TIMEOUT, callback::receive(&self.listener, &self.redirect_uri, &self.state, self.context.client_id.as_deref())) => result,
        }.map_err(|_| LoginFailure::new(LoginStage::Timeout, "等待浏览器授权超时，请重新登录"))??;
        self.issued_client = Some(result.client_id.clone());
        self.code = Some(result.code);
        Ok(result.client_id)
    }

    pub async fn exchange(
        mut self,
        cancel: &CancellationToken,
    ) -> Result<VerifiedLogin, LoginFailure> {
        let code = self
            .code
            .take()
            .ok_or_else(|| LoginFailure::new(LoginStage::Token, "尚未收到有效授权码"))?;
        let client_id = self
            .issued_client
            .as_ref()
            .ok_or_else(|| LoginFailure::new(LoginStage::Token, "缺少实际签发的客户端标识"))?;
        let token = async {
            let response = self
                .client
                .post(TOKEN_ENDPOINT)
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("client_id", client_id),
                    ("code", &code),
                    ("code_verifier", &self.verifier),
                    ("redirect_uri", &self.redirect_uri),
                    ("resource", RESOURCE),
                ])
                .send()
                .await
                .map_err(|_| {
                    LoginFailure::new(
                        LoginStage::Token,
                        "网络连接失败或请求超时，请重新授权，不重复兑换旧码",
                    )
                })?;
            parse_response::<TokenResponse>(response, LoginStage::Token).await
        };
        let response = tokio::select! { biased; _ = cancel.cancelled() => return Err(LoginFailure::cancelled()), result = token => result? };
        let received_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| LoginFailure::new(LoginStage::Token, "系统时钟无效"))?
            .as_secs();
        let claims = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(LoginFailure::cancelled()),
            result = identity::verify(&self.client, response.id_token.as_deref().ok_or_else(|| LoginFailure::new(LoginStage::Identity, "缺少 ID Token"))?, client_id, Some(&self.nonce)) => result?,
        };
        if self
            .context
            .subject
            .as_ref()
            .is_some_and(|subject| subject != &claims.sub)
        {
            return Err(LoginFailure::new(
                LoginStage::Identity,
                "返回账号与现有登记不一致，原会话保持不变",
            ));
        }
        let session = response.into_session(received_at, None)?;
        Ok(VerifiedLogin {
            record: AccountRecord {
                ext_agent_host_id: self.context.host_id,
                registration: Some(Registration {
                    issuer: ISSUER.into(),
                    client_id: client_id.clone(),
                    subject: claims.sub,
                    display_name: claims.name,
                    email: claims.email,
                }),
                session: Some(session),
            },
        })
    }
}

#[derive(Deserialize)]
pub(crate) struct TokenResponse {
    pub(crate) id_token: Option<String>,
    pub(crate) access_token: Option<String>,
    pub(crate) refresh_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    expires_in: Option<u64>,
    earliest_refresh_at: Option<serde_json::Value>,
}

impl TokenResponse {
    pub(crate) fn into_session(
        self,
        received_at: u64,
        previous: Option<&Session>,
    ) -> Result<Session, LoginFailure> {
        let invalid =
            || LoginFailure::new(LoginStage::Token, "服务返回的凭据、授权范围或有效期不完整");
        let expires_at = self
            .expires_in
            .map(|seconds| {
                received_at
                    .checked_add(seconds)
                    .filter(|_| seconds > 0)
                    .ok_or_else(invalid)
            })
            .transpose()?;
        let refresh_not_before = match self.earliest_refresh_at {
            None => None,
            Some(serde_json::Value::Number(value)) => Some(value.as_u64().ok_or_else(invalid)?),
            Some(serde_json::Value::String(value)) => Some(
                u64::try_from(
                    time::OffsetDateTime::parse(
                        &value,
                        &time::format_description::well_known::Rfc3339,
                    )
                    .map_err(|_| invalid())?
                    .unix_timestamp(),
                )
                .map_err(|_| invalid())?,
            ),
            Some(_) => return Err(invalid()),
        };
        let session = Session {
            identity_binding: previous.and_then(|session| session.identity_binding.clone()),
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            id_token: self
                .id_token
                .or_else(|| previous.map(|session| session.id_token.clone()))
                .ok_or_else(invalid)?,
            token_type: self.token_type,
            scopes: match self.scope {
                Some(scope) => scope.split_whitespace().map(str::to_owned).collect(),
                None => previous
                    .map(|session| session.scopes.clone())
                    .ok_or_else(invalid)?,
            },
            received_at,
            expires_at,
            refresh_not_before,
        };
        Ok(session)
    }
}

pub(crate) fn http_client() -> Result<reqwest::Client, LoginFailure> {
    reqwest::Client::builder()
        .user_agent(concat!("Agentelier/", env!("CARGO_PKG_VERSION")))
        .default_headers(reqwest::header::HeaderMap::from_iter([(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/json"),
        )]))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| LoginFailure::new(LoginStage::Prepare, "无法建立安全 HTTP 客户端"))
}

pub(super) async fn parse_response<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
    stage: LoginStage,
) -> Result<T, LoginFailure> {
    let status = response.status();
    let html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    let challenge = response
        .headers()
        .get("cf-mitigated")
        .is_some_and(|value| value == "challenge");
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| LoginFailure::new(stage, "无法读取授权服务响应"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(LoginFailure::new(stage, "授权服务响应超出大小限制"));
        }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        let value: Option<serde_json::Value> = serde_json::from_slice(&bytes).ok();
        let code = value
            .as_ref()
            .and_then(|value| value.get("error"))
            .and_then(|error| {
                error
                    .as_str()
                    .or_else(|| error.get("code").and_then(serde_json::Value::as_str))
            });
        let reason = match code {
            Some("unsupported_country_region_territory") => "当前网络出口所在地区不受支持",
            Some("invalid_grant") => "授权码已被拒绝，请开始新的登录",
            Some("invalid_client") => "实际客户端身份被拒绝",
            Some("access_denied") => "授权服务拒绝本次授权",
            Some(
                "chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context",
            ) => "当前授权范围或上下文不允许此操作",
            Some("subscription_sharing_user_not_eligible") => "当前账号或工作区不支持订阅授权",
            _ if challenge => "网络端返回了浏览器安全验证挑战",
            _ if html => "网络端返回 HTML 拒绝页面，而非 OAuth JSON 响应",
            _ if value.is_some() => "授权服务返回 JSON 拒绝响应",
            _ => "网络端返回非 JSON 拒绝响应",
        };
        return Err(LoginFailure::new(
            stage,
            format!("HTTP {}：{reason}；原会话保持不变", status.as_u16()),
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| LoginFailure::new(stage, "授权服务返回的字段或格式无效"))
}

fn random() -> Result<String, LoginFailure> {
    let mut bytes = [0u8; RANDOM_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|_| LoginFailure::new(LoginStage::Prepare, "无法取得安全随机数"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
