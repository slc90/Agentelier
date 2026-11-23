use crate::codex::{ISSUER, summarize_tokens, unix_time};
use crate::{CodexAccountSummary, CodexReadFailure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Instant, timeout, timeout_at};
use tokio_util::sync::CancellationToken;
use url::{Url, form_urlencoded};

mod refresh;
mod revoke;
mod store;
pub use refresh::RefreshFailure;
pub use revoke::RevocationFailure;
pub use store::{CredentialStore, CredentialStoreFailure};

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";
pub const CODEX_ORIGINATOR: &str = "agentelier";
const DEFAULT_PORT: u16 = 1455;
const FALLBACK_PORT: u16 = 1457;
const CALLBACK_PATH: &str = "/auth/callback";
const LOGIN_TIMEOUT: Duration = Duration::from_mins(5);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);
const TOKEN_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CALLBACK_BYTES: usize = 8 * 1024;
const MAX_CALLBACK_HEADERS: usize = 64;
const RANDOM_BYTES: usize = 32;
const URL_TOKEN_BYTES: usize = (RANDOM_BYTES * 4).div_ceil(3);

#[derive(Debug)]
pub enum OAuthFailure {
    Random,
    Listen(io::Error),
    InvalidUrl,
    Cancelled,
    Timeout,
    InvalidCallback,
    StaleCallback,
    StateMismatch,
    Denied,
    Provider,
    MissingCode,
    Exchange(reqwest::Error),
    Rejected(StatusCode),
    Credentials(CodexReadFailure),
}

impl fmt::Display for OAuthFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Random => f.write_str("无法为本次登录生成随机数据"),
            Self::Listen(error) => write!(f, "无法监听本地 OAuth 回调：{error}"),
            Self::InvalidUrl => f.write_str("无法构造 OAuth 授权地址"),
            Self::Cancelled => f.write_str("已取消浏览器登录"),
            Self::Timeout => f.write_str("等待浏览器授权超时"),
            Self::InvalidCallback => f.write_str("OAuth 回调参数重复或无效"),
            Self::StaleCallback => f.write_str("OAuth 回调属于另一登录操作"),
            Self::StateMismatch => f.write_str("OAuth 回调的 State 与本次登录不匹配"),
            Self::Denied => f.write_str("用户拒绝了浏览器授权"),
            Self::Provider => f.write_str("OAuth 服务未完成授权"),
            Self::MissingCode => f.write_str("OAuth 回调缺少授权码"),
            Self::Exchange(error) if error.is_timeout() => f.write_str("OAuth 令牌交换超时"),
            Self::Exchange(error) if error.is_decode() => f.write_str("OAuth 令牌响应格式无效"),
            Self::Exchange(_) => f.write_str("无法完成 OAuth 令牌交换"),
            Self::Rejected(status) => write!(f, "OAuth 服务拒绝令牌交换（HTTP {status}）"),
            Self::Credentials(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for OAuthFailure {}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OAuthTokens {
    #[serde(default)]
    id_token: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    expires_at: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CodexOAuthCredentials {
    tokens: OAuthTokens,
    summary: CodexAccountSummary,
}

impl CodexOAuthCredentials {
    fn from_response(response: TokenResponse) -> Result<Self, CodexReadFailure> {
        Self::from_tokens(OAuthTokens {
            id_token: response.id_token.ok_or(CodexReadFailure::MissingTokens)?,
            access_token: response
                .access_token
                .ok_or(CodexReadFailure::MissingTokens)?,
            refresh_token: response
                .refresh_token
                .ok_or(CodexReadFailure::MissingTokens)?,
            expires_at: match response.expires_in {
                Some(seconds) => unix_time()?.saturating_add(seconds),
                None => u64::MAX,
            },
        })
    }

    fn from_tokens(mut tokens: OAuthTokens) -> Result<Self, CodexReadFailure> {
        let (summary, claim_expiry) = summarize_tokens(
            &tokens.id_token,
            &tokens.access_token,
            &tokens.refresh_token,
        )?;
        tokens.expires_at = tokens.expires_at.min(claim_expiry);
        Ok(Self { tokens, summary })
    }

    pub fn summary(&self) -> &CodexAccountSummary {
        &self.summary
    }

    pub fn is_expired(&self) -> Result<bool, CodexReadFailure> {
        Ok(self.tokens.expires_at <= unix_time()?)
    }

    pub(crate) fn access_token(&self) -> &str {
        &self.tokens.access_token
    }
}

pub struct BrowserLogin {
    listener: TcpListener,
    authorization_url: Url,
    redirect_uri: String,
    state: String,
    verifier: String,
    deadline: Instant,
}

impl BrowserLogin {
    pub async fn start() -> Result<Self, OAuthFailure> {
        let state = format!("{}.{}", random_url_token()?, random_url_token()?);
        let verifier = random_url_token()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, DEFAULT_PORT)).await {
            Ok(listener) => listener,
            Err(_) => TcpListener::bind((Ipv4Addr::LOCALHOST, FALLBACK_PORT))
                .await
                .map_err(OAuthFailure::Listen)?,
        };
        let port = listener.local_addr().map_err(OAuthFailure::Listen)?.port();
        let redirect_uri = format!("http://localhost:{port}{CALLBACK_PATH}");
        let mut authorization_url = Url::parse(&format!("{ISSUER}/oauth/authorize"))
            .map_err(|_| OAuthFailure::InvalidUrl)?;
        authorization_url.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", &redirect_uri),
            ("scope", SCOPE),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("prompt", "select_account"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", CODEX_ORIGINATOR),
        ]);
        Ok(Self {
            listener,
            authorization_url,
            redirect_uri,
            state,
            verifier,
            deadline: Instant::now() + LOGIN_TIMEOUT,
        })
    }

    pub fn authorization_url(&self) -> &str {
        self.authorization_url.as_str()
    }

    pub async fn complete(
        self,
        cancellation: &CancellationToken,
    ) -> Result<CodexOAuthCredentials, OAuthFailure> {
        tokio::select! { biased;
            () = cancellation.cancelled() => Err(OAuthFailure::Cancelled),
            result = timeout_at(self.deadline, self.exchange()) => {
                result.map_err(|_| OAuthFailure::Timeout)?
            }
        }
    }

    async fn exchange(self) -> Result<CodexOAuthCredentials, OAuthFailure> {
        let code = wait_callback(self.listener, &self.state).await?;
        let response = token_client()?
            .post(format!("{ISSUER}/oauth/token"))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLIENT_ID),
                ("code", &code),
                ("code_verifier", &self.verifier),
                ("redirect_uri", &self.redirect_uri),
            ])
            .send()
            .await
            .map_err(OAuthFailure::Exchange)?;
        if !response.status().is_success() {
            return Err(OAuthFailure::Rejected(response.status()));
        }
        let tokens = response.json().await.map_err(OAuthFailure::Exchange)?;
        CodexOAuthCredentials::from_response(tokens).map_err(OAuthFailure::Credentials)
    }
}

fn token_client() -> Result<Client, OAuthFailure> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .timeout(TOKEN_TIMEOUT)
        .build()
        .map_err(OAuthFailure::Exchange)
}

fn random_url_token() -> Result<String, OAuthFailure> {
    let mut bytes = [0; RANDOM_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| OAuthFailure::Random)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

async fn wait_callback(listener: TcpListener, state: &str) -> Result<String, OAuthFailure> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(OAuthFailure::Listen)?;
        let Ok(Some(target)) = timeout(CONNECTION_TIMEOUT, request_target(&mut stream)).await
        else {
            continue;
        };
        let (path, query) = target.split_once('?').unwrap_or((&target, ""));
        if path != CALLBACK_PATH {
            drop(
                timeout(
                    CONNECTION_TIMEOUT,
                    reply(&mut stream, "404 Not Found", "Not found"),
                )
                .await,
            );
            continue;
        }
        let result = callback_code(query, state);
        let (status, body) = match &result {
            Ok(_) => (
                "200 OK",
                "Authorization received. Return to Agentelier to see the result.",
            ),
            Err(_) => (
                "400 Bad Request",
                "Authorization failed. Return to Agentelier to see the result.",
            ),
        };
        // Browser disconnects must not discard an authorization code already accepted by the app.
        drop(timeout(CONNECTION_TIMEOUT, reply(&mut stream, status, body)).await);
        if matches!(result, Err(OAuthFailure::StaleCallback)) {
            continue;
        }
        return result;
    }
}

async fn request_target(stream: &mut TcpStream) -> Option<String> {
    let mut bytes = [0; MAX_CALLBACK_BYTES];
    let mut used = 0;
    loop {
        let read = stream.read(&mut bytes[used..]).await.ok()?;
        if read == 0 {
            return None;
        }
        used += read;
        let mut headers = [httparse::EMPTY_HEADER; MAX_CALLBACK_HEADERS];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(&bytes[..used]) {
            Ok(httparse::Status::Complete(_)) if request.method == Some("GET") => {
                return request.path.map(str::to_owned);
            }
            Ok(httparse::Status::Partial) if used < bytes.len() => {}
            _ => return None,
        }
    }
}

fn callback_code(query: &str, expected_state: &str) -> Result<String, OAuthFailure> {
    let mut state = None;
    let mut code = None;
    let mut error = None;
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        let parameter = match key.as_ref() {
            "state" => &mut state,
            "code" => &mut code,
            "error" => &mut error,
            _ => continue,
        };
        if parameter.replace(value).is_some() {
            return Err(OAuthFailure::InvalidCallback);
        }
    }
    if state.as_deref() != Some(expected_state) {
        if let Some(attempt) = state.as_deref().and_then(callback_attempt)
            && callback_attempt(expected_state) != Some(attempt)
        {
            return Err(OAuthFailure::StaleCallback);
        }
        return Err(OAuthFailure::StateMismatch);
    }
    if let Some(error) = error {
        return Err(if error == "access_denied" {
            OAuthFailure::Denied
        } else {
            OAuthFailure::Provider
        });
    }
    code.filter(|code| !code.is_empty())
        .map(|code| code.into_owned())
        .ok_or(OAuthFailure::MissingCode)
}

fn callback_attempt(state: &str) -> Option<&str> {
    let (attempt, nonce) = state.split_once('.')?;
    [attempt, nonce]
        .into_iter()
        .all(|value| {
            value.len() == URL_TOKEN_BYTES
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .then_some(attempt)
}

async fn reply(stream: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await
}
