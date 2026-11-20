use crate::credentials::{CredentialClient, CredentialIntent};
use crate::source::{AccountSource, SourceChange, SourceSettings};
use agentelier_login::{SessionFailure, UsableCredentials};
use std::{error::Error, fmt};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub struct AccountEvent {
    pub operation_id: u64,
    pub kind: AccountEventKind,
}

pub enum AccountEventKind {
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
            Self::Task => f.write_str("认证文件任务异常结束"),
            Self::CredentialState => f.write_str("凭据共享请求状态锁已损坏"),
            Self::Disconnected => f.write_str("请选择有效的 Codex 凭据文件"),
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
    ChangeSource(u64, SourceChange),
    ReadSource,
    Credentials {
        current: CancellationToken,
        intent: CredentialIntent,
        result: watch::Sender<Option<Result<UsableCredentials, SessionFailure>>>,
    },
}

pub struct Account {
    credentials: CredentialClient,
    source: SourceSettings,
    snapshot: Option<AccountSource>,
}

impl Account {
    pub fn new(credentials: CredentialClient, source: SourceSettings) -> Self {
        Self {
            credentials,
            source,
            snapshot: None,
        }
    }

    pub fn codex_selected(&self) -> bool {
        self.source.codex_selected()
    }

    pub fn codex_identity(&self) -> Option<&crate::CodexIdentity> {
        self.snapshot
            .as_ref()
            .and_then(AccountSource::codex_identity)
    }

    pub async fn handle(
        &mut self,
        command: AccountCommand,
    ) -> Result<Option<AccountEvent>, AccountFailure> {
        let mut source = self.source.clone();
        match command {
            AccountCommand::ChangeSource(id, change) => {
                let (updated, result) = tokio::task::spawn_blocking(move || {
                    let result = source.change(change).map(|()| source.read());
                    (source, result)
                })
                .await
                .map_err(|_| AccountFailure::Task)?;
                match result {
                    Ok(snapshot) => {
                        self.source = updated;
                        self.observe(id, snapshot)
                    }
                    Err(_) => Ok(Some(AccountEvent {
                        operation_id: id,
                        kind: AccountEventKind::SourceChanged(Err("无法保存当前凭据来源".into())),
                    })),
                }
            }
            AccountCommand::ReadSource => {
                let snapshot = tokio::task::spawn_blocking(move || source.read())
                    .await
                    .map_err(|_| AccountFailure::Task)?;
                self.observe(0, snapshot)
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
                let (snapshot, outcome) =
                    tokio::task::spawn_blocking(move || source.acquire(intent))
                        .await
                        .map_err(|_| AccountFailure::Task)?;
                let event = self.observe(0, snapshot)?;
                result.send_replace(Some(outcome));
                Ok(event)
            }
        }
    }

    fn observe(
        &mut self,
        operation_id: u64,
        snapshot: AccountSource,
    ) -> Result<Option<AccountEvent>, AccountFailure> {
        let previous = self.snapshot.as_ref();
        let replaced = previous.is_some_and(|old| {
            old.path() != snapshot.path() || old.codex_identity() != snapshot.codex_identity()
        });
        self.credentials
            .update_source(self.source.codex_selected(), replaced)?;
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
