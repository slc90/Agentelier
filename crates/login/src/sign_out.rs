use crate::authentication::{AccountRecord, AuthFile, ISSUER};
use crate::browser::{LoginStage, http_client, parse_response};
use crate::session::RenewalState;
use std::time::Duration;
use std::{error::Error, fmt};
use url::Url;

const REVOCATION_ATTEMPTS: usize = 3;
const REVOCATION_TIMEOUT: Duration = Duration::from_secs(8);
const REVOCATION_BACKOFF: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
pub enum RemoteRevocation {
    Confirmed,
    Unconfirmed(String),
    NotApplicable,
}
#[derive(Clone, Copy, Debug)]
pub enum SignOutStage {
    Settling,
    Revoking,
    Clearing,
}
#[derive(Clone, Debug)]
pub struct SignOutFailure(String);
impl SignOutFailure {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
    pub fn local_clear(failure: impl fmt::Display) -> Self {
        Self::new(format!("退出未完成，无法清除本地凭据：{failure}"))
    }
}
impl fmt::Display for SignOutFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for SignOutFailure {}

pub(crate) enum SignOutState {
    Inactive,
    Pending(RemoteRevocation),
    Complete(RemoteRevocation),
}
pub enum SignOutPreparation {
    Revoke(Box<RevokeSession>),
    SavePending,
    Complete(RemoteRevocation),
}
pub struct RevokeSession {
    record: Option<AccountRecord>,
    generation: uuid::Uuid,
}
pub struct ClearedSession {
    record: Option<AccountRecord>,
    generation: uuid::Uuid,
    remote: RemoteRevocation,
}

impl AuthFile {
    pub fn prepare_sign_out(&mut self) -> Result<SignOutPreparation, SignOutFailure> {
        match &self.sign_out {
            SignOutState::Pending(_) => return Ok(SignOutPreparation::SavePending),
            SignOutState::Complete(remote) => {
                return Ok(SignOutPreparation::Complete(remote.clone()));
            }
            SignOutState::Inactive => {}
        }
        if self.pending.is_none() && self.record.is_none() {
            self.restore().map_err(|failure| {
                SignOutFailure::new(format!("退出未完成，无法读取本地凭据：{failure}"))
            })?;
        }
        Ok(SignOutPreparation::Revoke(Box::new(RevokeSession {
            record: self.pending.as_ref().or(self.record.as_ref()).cloned(),
            generation: self.generation,
        })))
    }
    pub fn stage_sign_out(&mut self, cleared: ClearedSession) -> Result<(), SignOutFailure> {
        if self.generation != cleared.generation {
            return Err(SignOutFailure::new("退出结果不属于当前会话代次"));
        }
        self.pending = cleared.record;
        self.renewal = RenewalState::Ready;
        self.issued_client = None;
        self.sign_out = if self.pending.is_some() {
            SignOutState::Pending(cleared.remote)
        } else {
            SignOutState::Complete(cleared.remote)
        };
        Ok(())
    }
    pub fn has_pending_sign_out(&self) -> bool {
        matches!(self.sign_out, SignOutState::Pending(_))
    }
    pub fn sign_out_result(&self) -> Option<RemoteRevocation> {
        match &self.sign_out {
            SignOutState::Complete(remote) => Some(remote.clone()),
            _ => None,
        }
    }
    pub(crate) fn signing_out_or_ended(&self) -> bool {
        !matches!(self.sign_out, SignOutState::Inactive)
    }
}

impl RevokeSession {
    pub async fn revoke(mut self) -> ClearedSession {
        let remote = match self
            .record
            .as_ref()
            .and_then(|record| record.session.as_ref())
        {
            None => RemoteRevocation::NotApplicable,
            Some(session) => match (
                session.refresh_token.as_deref(),
                self.record
                    .as_ref()
                    .and_then(|record| record.registration.as_ref()),
            ) {
                (Some(token), Some(registration))
                    if registration.client_id != "dynamic_agent_client" =>
                {
                    revoke_remote(token, &registration.client_id).await
                }
                _ => RemoteRevocation::Unconfirmed(
                    "没有可用的刷新凭据或实际客户端，未确认远端撤销".into(),
                ),
            },
        };
        if let Some(record) = &mut self.record {
            record.session = None;
        }
        ClearedSession {
            record: self.record,
            generation: self.generation,
            remote,
        }
    }
}

#[derive(serde::Deserialize)]
struct Discovery {
    issuer: String,
    revocation_endpoint: String,
}
struct RevokeFailure {
    retryable: bool,
    message: String,
}
impl RevokeFailure {
    fn new(retryable: bool, message: impl Into<String>) -> Self {
        Self {
            retryable,
            message: message.into(),
        }
    }
}
async fn revoke_remote(token: &str, client_id: &str) -> RemoteRevocation {
    let client = match http_client() {
        Ok(client) => client,
        Err(_) => return RemoteRevocation::Unconfirmed("无法建立安全撤销客户端".into()),
    };
    let mut last = "未确认远端撤销".to_owned();
    for attempt in 0..REVOCATION_ATTEMPTS {
        match revoke_once(&client, token, client_id).await {
            Ok(()) => return RemoteRevocation::Confirmed,
            Err(failure) => {
                tracing::debug!(attempt = attempt + 1, retryable = failure.retryable, error = %failure.message, "本次远端撤销未获确认");
                last = failure.message;
                if !failure.retryable || attempt + 1 == REVOCATION_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(REVOCATION_BACKOFF * (1 << attempt)).await;
            }
        }
    }
    RemoteRevocation::Unconfirmed(last)
}
async fn revoke_once(
    client: &reqwest::Client,
    token: &str,
    client_id: &str,
) -> Result<(), RevokeFailure> {
    let response = client
        .get(format!("{ISSUER}/.well-known/openid-configuration"))
        .timeout(REVOCATION_TIMEOUT)
        .send()
        .await
        .map_err(|_| RevokeFailure::new(true, "无法连接撤销发现服务或请求超时"))?;
    if !response.status().is_success() {
        return Err(RevokeFailure::new(
            response.status().is_server_error(),
            format!("撤销发现服务返回 HTTP {}", response.status().as_u16()),
        ));
    }
    let discovery: Discovery = parse_response(response, LoginStage::Identity)
        .await
        .map_err(|_| RevokeFailure::new(true, "撤销发现配置格式无效或无法读取"))?;
    let endpoint = Url::parse(&discovery.revocation_endpoint)
        .map_err(|_| RevokeFailure::new(false, "撤销地址无效"))?;
    if discovery.issuer != ISSUER
        || endpoint.scheme() != "https"
        || endpoint.host_str() != Some("auth.openai.com")
        || endpoint.port_or_known_default() != Some(443)
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(RevokeFailure::new(false, "固定签发方的撤销地址校验失败"));
    }
    let response = client
        .post(endpoint)
        .timeout(REVOCATION_TIMEOUT)
        .form(&[
            ("token", token),
            ("token_type_hint", "refresh_token"),
            ("client_id", client_id),
        ])
        .send()
        .await
        .map_err(|_| RevokeFailure::new(true, "无法连接撤销服务或请求超时"))?;
    if response.status() == reqwest::StatusCode::OK {
        return Ok(());
    }
    Err(RevokeFailure::new(
        response.status().is_server_error(),
        format!("撤销服务返回 HTTP {}", response.status().as_u16()),
    ))
}
