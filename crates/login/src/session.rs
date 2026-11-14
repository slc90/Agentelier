use crate::authentication::{
    AccountRecord, AuthFile, CredentialState, IdentityBinding, RestoredAccount, SUBSCRIPTION_SCOPE,
    Session,
};
use crate::browser::{
    MAX_RESPONSE_BYTES, RESOURCE, TOKEN_ENDPOINT, TokenResponse, http_client, identity,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{error::Error, fmt};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const REFRESH_WINDOW_SECONDS: u64 = 60;
const TEMPORARY_RETRY_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionActivity {
    Idle,
    Checking,
    Refreshing,
    Saving,
    SaveFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionFailureKind {
    SignedOut,
    SubscriptionMissing,
    NotYetRefreshable,
    Temporary,
    Uncertain,
    ReauthenticationRequired,
    ClientConfiguration,
    Storage,
    Identity,
    Clock,
    Busy,
    Runtime,
}

#[derive(Clone, Debug)]
pub struct SessionFailure {
    pub kind: SessionFailureKind,
    pub retry_at: Option<u64>,
    message: String,
}
impl SessionFailure {
    pub fn new(kind: SessionFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            retry_at: None,
            message: message.into(),
        }
    }
    pub(crate) fn reauthentication() -> Self {
        Self::new(
            SessionFailureKind::ReauthenticationRequired,
            "会话已失效，请重新登录；账号登记保持不变",
        )
    }
    fn uncertain() -> Self {
        Self::new(
            SessionFailureKind::Uncertain,
            "续期结果无法确定，已停止重复使用旧刷新凭据；请重新登录",
        )
    }
}
impl fmt::Display for SessionFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for SessionFailure {}

#[derive(Clone, PartialEq, Eq)]
pub struct CredentialVersion {
    client_id: String,
    subject: String,
    generation: Uuid,
}

#[derive(Clone)]
pub struct UsableCredentials {
    access_token: String,
    version: CredentialVersion,
    pub expires_at: u64,
}
impl UsableCredentials {
    pub fn access_token(&self) -> &str {
        &self.access_token
    }
    pub fn version(&self) -> &CredentialVersion {
        &self.version
    }
}

pub struct AuthenticatedRequest {
    version: CredentialVersion,
    recovery_used: bool,
    output_started: bool,
}
impl AuthenticatedRequest {
    pub fn new(credentials: &UsableCredentials) -> Self {
        Self {
            version: credentials.version.clone(),
            recovery_used: false,
            output_started: false,
        }
    }
    pub fn mark_output_started(&mut self) {
        self.output_started = true;
    }
    pub fn begin_recovery(&mut self, status: u16) -> Option<CredentialVersion> {
        if status != 401 || self.recovery_used || self.output_started {
            return None;
        }
        self.recovery_used = true;
        Some(self.version.clone())
    }
}

pub(crate) enum RenewalState {
    Ready,
    Failed(SessionFailure),
    Pending { invalid: bool },
}

pub enum CredentialPreparation {
    Available(UsableCredentials),
    Refresh(Box<RefreshAttempt>),
    SavePending,
}
pub struct RefreshAttempt {
    record: AccountRecord,
}
pub struct RefreshedSession {
    record: AccountRecord,
}

impl AuthFile {
    pub fn session_snapshot(&self) -> Result<RestoredAccount, SessionFailure> {
        let mut snapshot = match &self.record {
            Some(record) => record.projection().map_err(|_| {
                SessionFailure::new(SessionFailureKind::Clock, "无法判断账号凭据状态")
            })?,
            None => RestoredAccount {
                account: None,
                credentials: CredentialState::NoRecord,
            },
        };
        if matches!(self.renewal, RenewalState::Pending { invalid: true })
            || matches!(&self.renewal, RenewalState::Failed(failure) if failure.kind == SessionFailureKind::ReauthenticationRequired)
        {
            snapshot.credentials = CredentialState::ReauthenticationRequired;
        }
        Ok(snapshot)
    }

    pub fn prepare_credentials(
        &mut self,
        rejected: Option<&CredentialVersion>,
    ) -> Result<CredentialPreparation, SessionFailure> {
        if self.signing_out_or_ended() {
            return Err(SessionFailure::new(
                SessionFailureKind::SignedOut,
                "账号正在退出或已退出，需要重新登录",
            ));
        }
        if rejected.is_some_and(|version| {
            self.record
                .as_ref()
                .and_then(|record| record.registration.as_ref())
                .is_none_or(|registration| {
                    version.client_id != registration.client_id
                        || version.subject != registration.subject
                })
        }) {
            return Err(SessionFailure::new(
                SessionFailureKind::Identity,
                "请求账号与当前登记不同，不能重发原请求",
            ));
        }
        if matches!(self.renewal, RenewalState::Pending { .. }) {
            return Ok(CredentialPreparation::SavePending);
        }
        if self.pending.is_some() {
            return Err(SessionFailure::new(
                SessionFailureKind::Busy,
                "仍有待保存的浏览器授权结果",
            ));
        }
        let now = now()?;
        if let RenewalState::Failed(failure) = &self.renewal
            && failure.retry_at.is_none_or(|retry| now < retry)
        {
            return Err(failure.clone());
        }
        self.renewal = RenewalState::Ready;
        let record = self
            .record
            .as_ref()
            .ok_or_else(|| SessionFailure::new(SessionFailureKind::SignedOut, "尚未登录"))?;
        if record.registration.is_none() {
            return Err(SessionFailure::new(
                SessionFailureKind::SignedOut,
                "尚未建立账号登记",
            ));
        }
        let session = record
            .session
            .as_ref()
            .ok_or_else(SessionFailure::reauthentication)?;
        if !session
            .scopes
            .iter()
            .any(|scope| scope == SUBSCRIPTION_SCOPE)
        {
            return Err(SessionFailure::new(
                SessionFailureKind::SubscriptionMissing,
                "账号已登录，订阅授权未开启",
            ));
        }
        let expires = session
            .expires_at
            .ok_or_else(SessionFailure::reauthentication)?;
        let valid = now < expires;
        let updated = rejected.is_some_and(|version| version.generation != self.generation);
        let due = expires.saturating_sub(now) <= REFRESH_WINDOW_SECONDS;
        let refresh_allowed = session
            .refresh_not_before
            .is_none_or(|earliest| now >= earliest);
        if valid
            && (updated
                || (rejected.is_none()
                    && (!due || !refresh_allowed || session.refresh_token.is_none())))
        {
            return self
                .usable_credentials()
                .map(CredentialPreparation::Available);
        }
        if !refresh_allowed {
            let mut failure = SessionFailure::new(
                SessionFailureKind::NotYetRefreshable,
                "凭据当前不可用，尚未到服务允许的最早续期时间",
            );
            failure.retry_at = session.refresh_not_before;
            return Err(failure);
        }
        if session.refresh_token.is_none() {
            return Err(SessionFailure::reauthentication());
        }
        Ok(CredentialPreparation::Refresh(Box::new(RefreshAttempt {
            record: record.clone(),
        })))
    }

    pub fn usable_credentials(&self) -> Result<UsableCredentials, SessionFailure> {
        if self.signing_out_or_ended() {
            return Err(SessionFailure::new(
                SessionFailureKind::SignedOut,
                "账号正在退出或已退出，需要重新登录",
            ));
        }
        let record = self
            .record
            .as_ref()
            .ok_or_else(SessionFailure::reauthentication)?;
        let registration = record
            .registration
            .as_ref()
            .ok_or_else(SessionFailure::reauthentication)?;
        let session = record
            .session
            .as_ref()
            .ok_or_else(SessionFailure::reauthentication)?;
        if !matches!(self.renewal, RenewalState::Ready) || self.pending.is_some() {
            return Err(SessionFailure::new(
                SessionFailureKind::Busy,
                "凭据变更尚未完成，不能交付旧记录",
            ));
        }
        if !session
            .scopes
            .iter()
            .any(|scope| scope == SUBSCRIPTION_SCOPE)
        {
            return Err(SessionFailure::new(
                SessionFailureKind::SubscriptionMissing,
                "账号已登录，订阅授权未开启",
            ));
        }
        let current_time = now()?;
        let expires_at = session
            .expires_at
            .filter(|expiry| *expiry > current_time)
            .ok_or_else(SessionFailure::reauthentication)?;
        Ok(UsableCredentials {
            access_token: session
                .access_token
                .clone()
                .ok_or_else(SessionFailure::reauthentication)?,
            version: CredentialVersion {
                client_id: registration.client_id.clone(),
                subject: registration.subject.clone(),
                generation: self.generation,
            },
            expires_at,
        })
    }

    pub fn stage_refresh(&mut self, update: RefreshedSession) -> Result<(), SessionFailure> {
        let current = self
            .record
            .as_ref()
            .ok_or_else(SessionFailure::reauthentication)?;
        if current.ext_agent_host_id != update.record.ext_agent_host_id
            || current
                .registration
                .as_ref()
                .map(|r| (&r.client_id, &r.subject))
                != update
                    .record
                    .registration
                    .as_ref()
                    .map(|r| (&r.client_id, &r.subject))
        {
            return Err(SessionFailure::new(
                SessionFailureKind::Identity,
                "续期结果不属于当前账号登记",
            ));
        }
        self.pending = Some(update.record);
        self.renewal = RenewalState::Pending { invalid: false };
        Ok(())
    }

    pub fn record_refresh_failure(&mut self, failure: SessionFailure) {
        if failure.kind == SessionFailureKind::ReauthenticationRequired
            && let Some(mut record) = self.record.clone()
        {
            record.session = None;
            self.pending = Some(record);
            self.renewal = RenewalState::Pending { invalid: true };
            return;
        }
        self.renewal = RenewalState::Failed(failure);
    }
    pub fn has_pending_session_update(&self) -> bool {
        matches!(self.renewal, RenewalState::Pending { .. })
    }
    pub fn session_failure(&self) -> Option<SessionFailure> {
        match &self.renewal {
            RenewalState::Failed(failure) => Some(failure.clone()),
            _ => None,
        }
    }
}

impl RefreshAttempt {
    pub async fn execute(mut self) -> Result<RefreshedSession, SessionFailure> {
        let registration = self
            .record
            .registration
            .as_ref()
            .ok_or_else(SessionFailure::reauthentication)?;
        if registration.client_id == "dynamic_agent_client" {
            return Err(SessionFailure::new(
                SessionFailureKind::ClientConfiguration,
                "账号登记没有实际签发的客户端标识",
            ));
        }
        let previous = self
            .record
            .session
            .as_ref()
            .ok_or_else(SessionFailure::reauthentication)?;
        let refresh = previous
            .refresh_token
            .as_deref()
            .ok_or_else(SessionFailure::reauthentication)?;
        let previous_claims = previous
            .identity_binding
            .clone()
            .map(Ok)
            .unwrap_or_else(|| original_claims(previous))?;
        let client = http_client().map_err(|_| {
            SessionFailure::new(
                SessionFailureKind::ClientConfiguration,
                "无法建立安全续期客户端",
            )
        })?;
        let response = client
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", registration.client_id.as_str()),
                ("refresh_token", refresh),
                ("resource", RESOURCE),
            ])
            .send()
            .await
            .map_err(|error| {
                if error.is_connect() {
                    temporary("无法连接续期服务，稍后可重试")
                } else {
                    SessionFailure::uncertain()
                }
            })?;
        let received_at = now()?;
        let token = read_response(response).await?;
        if token
            .access_token
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
            || token
                .refresh_token
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
        {
            return Err(SessionFailure::uncertain());
        }
        if let Some(id_token) = &token.id_token {
            let claims = identity::verify(&client, id_token, &registration.client_id, None)
                .await
                .map_err(|_| SessionFailure::uncertain())?;
            if claims.sub != registration.subject
                || claims.nonce.as_ref().is_some_and(|nonce| {
                    previous_claims
                        .nonce
                        .as_ref()
                        .is_none_or(|old| !bool::from(nonce.as_bytes().ct_eq(old.as_bytes())))
                })
                || claims
                    .auth_time
                    .is_some_and(|time| Some(time) != previous_claims.auth_time)
                || claims.aud != previous_claims.aud
            {
                return Err(SessionFailure::new(
                    SessionFailureKind::Identity,
                    "续期返回的身份与原始授权不一致，已停止使用该组凭据",
                ));
            }
        }
        let mut session = token
            .into_session(received_at, Some(previous))
            .map_err(|_| SessionFailure::uncertain())?;
        session.identity_binding = Some(previous_claims);
        self.record.session = Some(session);
        self.record
            .validate()
            .map_err(|_| SessionFailure::uncertain())?;
        Ok(RefreshedSession {
            record: self.record,
        })
    }
}

fn original_claims(session: &Session) -> Result<IdentityBinding, SessionFailure> {
    #[derive(serde::Deserialize)]
    struct Claims {
        nonce: Option<String>,
        auth_time: Option<u64>,
        aud: serde_json::Value,
    }
    let payload = session
        .id_token
        .split('.')
        .nth(1)
        .ok_or_else(SessionFailure::uncertain)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| SessionFailure::uncertain())?;
    let claims: Claims = serde_json::from_slice(&bytes).map_err(|_| SessionFailure::uncertain())?;
    Ok(IdentityBinding {
        nonce: claims.nonce,
        auth_time: claims.auth_time,
        aud: claims.aud,
    })
}
fn now() -> Result<u64, SessionFailure> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| {
            SessionFailure::new(
                SessionFailureKind::Clock,
                "系统时钟无效，无法判断凭据有效时间",
            )
        })
}
fn temporary(message: &str) -> SessionFailure {
    let mut failure = SessionFailure::new(SessionFailureKind::Temporary, message);
    failure.retry_at = now()
        .ok()
        .map(|time| time.saturating_add(TEMPORARY_RETRY_SECONDS));
    failure
}
async fn read_response(mut response: reqwest::Response) -> Result<TokenResponse, SessionFailure> {
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| SessionFailure::uncertain())?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(SessionFailure::uncertain());
        }
        bytes.extend_from_slice(&chunk);
    }
    if status.is_success() {
        return serde_json::from_slice(&bytes).map_err(|_| SessionFailure::uncertain());
    }
    let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
    let error = value.as_ref().and_then(|value| value.get("error"));
    let code = error.and_then(|error| {
        error
            .as_str()
            .or_else(|| error.get("code").and_then(serde_json::Value::as_str))
    });
    match code {
        Some(
            "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused",
        ) => Err(SessionFailure::reauthentication()),
        Some("invalid_client") => Err(SessionFailure::new(
            SessionFailureKind::ClientConfiguration,
            "续期客户端配置被拒绝，请检查客户端登记，重复登录无法修复该配置错误",
        )),
        _ if status == reqwest::StatusCode::TOO_MANY_REQUESTS => {
            Err(temporary("续期服务暂时限制请求，稍后可重试"))
        }
        _ => Err(SessionFailure::uncertain()),
    }
}
