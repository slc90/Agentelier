use crate::credentials::{CredentialClient, CredentialIntent};
use crate::source::AccountSource;
use agentelier_login::{
    BrowserLogin, CodexOAuthCredentials, CredentialStore, DataEnvironment, SessionFailure,
    UsableCredentials,
};
use std::{error::Error, fmt};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub struct AccountEvent {
    pub operation_id: u64,
    pub kind: AccountEventKind,
}

pub struct LoginPrompt {
    pub authorization_url: String,
    pub browser_error: Option<String>,
}

pub enum AccountEventKind {
    LoginWaiting(LoginPrompt),
    SourceSettling,
    SourceChanged(Result<AccountSource, String>),
}

#[derive(Clone, Copy, Debug)]
pub enum DeliveryFailure {
    Closed,
    Full,
    Wake,
}

#[derive(Debug)]
pub enum AccountFailure {
    Busy,
    Closed,
    Task,
    Delivery(DeliveryFailure),
    CredentialState,
    Disconnected,
    SourceSettling,
}

impl fmt::Display for AccountFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("认证后台命令队列已满"),
            Self::Closed => f.write_str("认证后台已停止接收命令"),
            Self::Task => f.write_str("认证后台任务异常结束"),
            Self::CredentialState => f.write_str("凭据共享请求状态锁已损坏"),
            Self::Disconnected => f.write_str("请先使用 ChatGPT 登录"),
            Self::SourceSettling => f.write_str("正在回收旧账号的任务并保存待办"),
            Self::Delivery(DeliveryFailure::Closed) => f.write_str("认证结果接收端已关闭"),
            Self::Delivery(DeliveryFailure::Full) => f.write_str("认证结果队列已满"),
            Self::Delivery(DeliveryFailure::Wake) => {
                f.write_str("认证结果已入队，但无法唤醒应用窗口")
            }
        }
    }
}
impl Error for AccountFailure {}

pub enum AccountCommand {
    ReadSource,
    CompleteLogin(u64, Result<CodexOAuthCredentials, String>),
    Credentials {
        current: CancellationToken,
        intent: CredentialIntent,
        result: watch::Sender<Option<Result<UsableCredentials, SessionFailure>>>,
    },
}

pub struct Account {
    credentials: CredentialClient,
    store: CredentialStore,
    current: Option<CodexOAuthCredentials>,
    snapshot: Option<AccountSource>,
}

impl Account {
    pub fn new(credentials: CredentialClient, environment: DataEnvironment) -> Self {
        Self {
            credentials,
            store: CredentialStore::new(environment),
            current: None,
            snapshot: None,
        }
    }

    pub fn codex_identity(&self) -> Option<&crate::CodexIdentity> {
        self.snapshot
            .as_ref()
            .and_then(AccountSource::codex_identity)
    }

    pub async fn browser_login(
        cancellation: &CancellationToken,
        prompt: impl FnOnce(LoginPrompt),
    ) -> Result<CodexOAuthCredentials, String> {
        let login = BrowserLogin::start()
            .await
            .map_err(|failure| failure.to_string())?;
        let browser_error = tokio::select! { biased;
            () = cancellation.cancelled() => return Err("已取消浏览器登录".into()),
            opened = login.open_browser() => opened.err().map(|failure| failure.to_string()),
        };
        prompt(LoginPrompt {
            authorization_url: login.authorization_url().to_owned(),
            browser_error,
        });
        login
            .complete(cancellation)
            .await
            .map_err(|failure| failure.to_string())
    }

    pub async fn handle(
        &mut self,
        command: AccountCommand,
    ) -> Result<Option<AccountEvent>, AccountFailure> {
        match command {
            AccountCommand::ReadSource => {
                let store = self.store.clone();
                let loaded = tokio::task::spawn_blocking(move || store.load())
                    .await
                    .map_err(|_| AccountFailure::Task)?;
                let snapshot = match loaded {
                    Ok(credentials) => {
                        self.current = credentials;
                        self.current
                            .as_ref()
                            .map_or(AccountSource::Disconnected, |current| {
                                AccountSource::Codex(current.summary().clone())
                            })
                    }
                    Err(failure) => AccountSource::Unavailable(failure.to_string()),
                };
                self.observe(0, snapshot, false)
            }
            AccountCommand::CompleteLogin(id, result) => {
                let result = match result {
                    Ok(credentials) => {
                        let store = self.store.clone();
                        tokio::task::spawn_blocking(move || {
                            store.save(&credentials).map(|()| credentials)
                        })
                        .await
                        .map_err(|_| AccountFailure::Task)?
                        .map_err(|failure| failure.to_string())
                    }
                    Err(failure) => Err(failure),
                };
                match result {
                    Ok(credentials) => {
                        let snapshot = AccountSource::Codex(credentials.summary().clone());
                        self.current = Some(credentials);
                        self.observe(id, snapshot, true)
                    }
                    Err(failure) => Ok(Some(AccountEvent {
                        operation_id: id,
                        kind: AccountEventKind::SourceChanged(Err(failure)),
                    })),
                }
            }
            AccountCommand::Credentials {
                current,
                intent,
                result,
            } => {
                if current.is_cancelled() {
                    result.send_replace(Some(Err(SessionFailure::new(
                        "凭据来源已变化，原请求已停止",
                    ))));
                    return Ok(None);
                }
                let Some(credentials) = self.current.as_ref() else {
                    result.send_replace(Some(Err(SessionFailure::new("请先使用 ChatGPT 登录"))));
                    return Ok(None);
                };
                let mut summary = credentials.summary().clone();
                let outcome = self.acquire(credentials, intent);
                if let Ok(expired) = credentials.is_expired() {
                    summary.expired = expired;
                }
                let event = self.observe(0, AccountSource::Codex(summary), false)?;
                result.send_replace(Some(outcome));
                Ok(event)
            }
        }
    }

    fn acquire(
        &self,
        credentials: &CodexOAuthCredentials,
        intent: CredentialIntent,
    ) -> Result<UsableCredentials, SessionFailure> {
        let current = UsableCredentials::from_oauth(credentials)?;
        if let CredentialIntent::Recover(rejected) = intent {
            if rejected.identity() != current.version().identity() {
                return Err(SessionFailure::new(
                    "Codex 凭据的账号或工作区已变化，原请求已停止",
                ));
            }
            if &rejected == current.version() {
                return Err(SessionFailure::new("Codex OAuth 凭据未更新，请重新登录"));
            }
        }
        Ok(current)
    }

    fn observe(
        &mut self,
        operation_id: u64,
        snapshot: AccountSource,
        new_login: bool,
    ) -> Result<Option<AccountEvent>, AccountFailure> {
        let previous = self.snapshot.as_ref();
        let replaced = new_login
            || previous.is_some_and(|old| old.codex_identity() != snapshot.codex_identity());
        self.credentials
            .update_source(matches!(snapshot, AccountSource::Codex(_)), replaced)?;
        if operation_id == 0 && previous == Some(&snapshot) {
            return Ok(None);
        }
        self.snapshot = Some(snapshot.clone());
        Ok(Some(AccountEvent {
            operation_id,
            kind: AccountEventKind::SourceChanged(Ok(snapshot)),
        }))
    }
}
