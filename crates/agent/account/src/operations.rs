use crate::credentials::{CredentialClient, CredentialIntent};
use crate::source::{AccountSource, SourceChange, SourceSettings};
use agentelier_login::{
    AuthFile, BrowserLogin, CredentialPreparation, LoginFailure, LoginStage, RemoteRevocation,
    RestoreFailure, RestoredAccount, SessionActivity, SessionFailure, SessionFailureKind,
    SignOutFailure, SignOutPreparation, SignOutStage, UsableCredentials,
};
use std::path::PathBuf;
use std::{error::Error, fmt};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

pub struct AccountEvent {
    pub operation_id: u64,
    pub kind: AccountEventKind,
}
pub enum AccountEventKind {
    SourceChanged(Result<AccountSource, String>),
    SignOutProgress(SignOutStage),
    SignOutFailed(SignOutFailure),
    SignedOut {
        snapshot: RestoredAccount,
        remote: RemoteRevocation,
    },
    SessionStatus {
        snapshot: RestoredAccount,
        activity: SessionActivity,
        failure: Option<SessionFailure>,
    },
    BrowserReady(String),
    Exchanging,
    Saving,
    SaveFailed(LoginFailure),
    Completed(Result<RestoredAccount, OperationFailure>),
    Cancelled,
}

#[derive(Debug)]
pub enum OperationFailure {
    Restore(RestoreFailure),
    Login(LoginFailure),
}
impl fmt::Display for OperationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Restore(error) => error.fmt(f),
            Self::Login(error) => error.fmt(f),
        }
    }
}
impl Error for OperationFailure {}

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
    Storage(String),
    SignedOut,
    SigningOut,
}
impl fmt::Display for AccountFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("认证后台命令队列已满"),
            Self::Closed => f.write_str("认证后台已停止接收命令"),
            Self::Task => f.write_str("认证文件任务异常结束"),
            Self::CredentialState => f.write_str("凭据共享请求状态锁已损坏"),
            Self::Storage(message) => f.write_str(message),
            Self::SignedOut => f.write_str("账号正在退出或已退出，需要重新登录"),
            Self::SigningOut => f.write_str("退出尚未完成，禁止新的账号操作"),
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
    SignOut(u64),
    Credentials {
        codex: bool,
        intent: CredentialIntent,
        result: watch::Sender<Option<Result<UsableCredentials, SessionFailure>>>,
    },
    Restore(u64),
    Login {
        id: u64,
        consent: bool,
        cancel: CancellationToken,
    },
    RetrySave(u64),
    Cancel(u64),
}

impl AccountCommand {
    pub fn reject_for_sign_out(self) {
        if let Self::Credentials { result, .. } = self {
            result.send_replace(Some(Err(SessionFailure::new(
                SessionFailureKind::SignedOut,
                "退出已取消尚未开始的凭据请求",
            ))));
        }
    }
}

pub struct Account {
    file: AuthFile,
    pending_save: Option<(u64, CancellationToken)>,
    credentials: CredentialClient,
    source: SourceSettings,
    source_snapshot: Option<AccountSource>,
}
impl Account {
    pub fn new(path: PathBuf, credentials: CredentialClient, source: SourceSettings) -> Self {
        Self {
            file: AuthFile::new(path),
            pending_save: None,
            credentials,
            source,
            source_snapshot: None,
        }
    }
    pub fn codex_selected(&self) -> bool {
        self.source.codex_selected()
    }

    pub fn codex_identity(&self) -> Option<&crate::CodexIdentity> {
        self.source_snapshot
            .as_ref()
            .and_then(AccountSource::codex_identity)
    }

    pub async fn handle(
        self,
        command: AccountCommand,
        publish: &impl Fn(AccountEvent) -> Result<(), DeliveryFailure>,
    ) -> Result<(Self, Option<AccountFailure>), AccountFailure> {
        let Self {
            mut file,
            mut pending_save,
            credentials: owner_credentials,
            mut source,
            mut source_snapshot,
        } = self;
        let mut delivery_failure = None;
        match command {
            AccountCommand::ChangeSource(id, change) => {
                let (updated, result) = tokio::task::spawn_blocking(move || {
                    let result = source.change(change).map(|()| source.read());
                    (source, result)
                })
                .await
                .map_err(|_| AccountFailure::Task)?;
                source = updated;
                if let Ok(snapshot) = &result {
                    let changed = source_snapshot.as_ref().is_some_and(|previous| {
                        previous.path() != snapshot.path()
                            || previous.codex_identity() != snapshot.codex_identity()
                    });
                    owner_credentials.update_source(source.codex_selected(), changed)?;
                    source_snapshot = Some(snapshot.clone());
                }
                let result = result.map_err(|_| "无法保存当前凭据来源".to_owned());
                capture_delivery(
                    emit(&publish, id, AccountEventKind::SourceChanged(result)),
                    &mut delivery_failure,
                );
            }
            AccountCommand::ReadSource => {
                read_source(
                    &source,
                    &owner_credentials,
                    &mut source_snapshot,
                    &publish,
                    &mut delivery_failure,
                )
                .await?;
            }
            AccountCommand::SignOut(id) => {
                pending_save = None;
                let (returned, failure) = sign_out(file, id, &owner_credentials, &publish).await?;
                file = returned;
                if delivery_failure.is_none() {
                    delivery_failure = failure;
                }
            }
            AccountCommand::Credentials { codex, result, .. }
                if codex != source.codex_selected() =>
            {
                result.send_replace(Some(Err(SessionFailure::new(
                    SessionFailureKind::Identity,
                    "凭据来源已变化，原请求已停止",
                ))));
            }
            AccountCommand::Credentials {
                codex: true,
                intent,
                result,
            } => {
                let selection = source.clone();
                let (snapshot, outcome) =
                    tokio::task::spawn_blocking(move || selection.acquire(intent))
                        .await
                        .map_err(|_| AccountFailure::Task)??;
                update_source_snapshot(
                    snapshot,
                    &owner_credentials,
                    &mut source_snapshot,
                    &publish,
                    &mut delivery_failure,
                )?;
                result.send_replace(Some(outcome));
            }
            AccountCommand::Credentials { intent, result, .. } => {
                let (returned, outcome, failure) = acquire(file, intent, &publish).await?;
                file = returned;
                result.send_replace(Some(outcome));
                if failure.is_some() {
                    delivery_failure = failure;
                }
            }
            AccountCommand::Restore(id) => {
                if source_snapshot.is_none() {
                    read_source(
                        &source,
                        &owner_credentials,
                        &mut source_snapshot,
                        &publish,
                        &mut delivery_failure,
                    )
                    .await?;
                }
                debug!(operation_id = id, "开始恢复当前目录账号");
                let (returned, result) = blocking(file, AuthFile::restore).await?;
                file = returned;
                let restored = result.is_ok();
                capture_delivery(
                    emit(
                        &publish,
                        id,
                        AccountEventKind::Completed(result.map_err(OperationFailure::Restore)),
                    ),
                    &mut delivery_failure,
                );
                if restored {
                    capture_delivery(
                        session_status(
                            &file,
                            SessionActivity::Idle,
                            file.session_failure(),
                            &publish,
                        ),
                        &mut delivery_failure,
                    );
                }
            }
            AccountCommand::Login {
                id,
                consent,
                cancel,
            } => {
                let (returned, pending, failure) =
                    login(file, id, consent, &cancel, &publish).await?;
                file = returned;
                pending_save = pending;
                if delivery_failure.is_none() {
                    delivery_failure = failure;
                }
                if pending_save.is_none()
                    && matches!(
                        file.session_snapshot()
                            .map_err(|failure| AccountFailure::Storage(failure.to_string()))?
                            .credentials,
                        agentelier_login::CredentialState::StoredSession { .. }
                    )
                {
                    owner_credentials.resume_after_login()?;
                }
            }
            AccountCommand::RetrySave(id)
                if pending_save
                    .as_ref()
                    .is_some_and(|(pending, _)| *pending == id) =>
            {
                if pending_save
                    .as_ref()
                    .is_some_and(|(_, token): &(u64, CancellationToken)| token.is_cancelled())
                {
                    file.discard_pending()
                        .map_err(|failure| AccountFailure::Storage(failure.to_string()))?;
                    pending_save = None;
                    capture_delivery(
                        emit(&publish, id, AccountEventKind::Cancelled),
                        &mut delivery_failure,
                    );
                    return Ok((
                        Self {
                            file,
                            pending_save,
                            credentials: owner_credentials,
                            source,
                            source_snapshot,
                        },
                        delivery_failure,
                    ));
                }
                capture_delivery(
                    emit(&publish, id, AccountEventKind::Saving),
                    &mut delivery_failure,
                );
                let (returned, result) = blocking(file, AuthFile::save_pending).await?;
                file = returned;
                match result {
                    Ok(snapshot) => {
                        pending_save = None;
                        owner_credentials.resume_after_login()?;
                        capture_delivery(
                            emit(&publish, id, AccountEventKind::Completed(Ok(snapshot))),
                            &mut delivery_failure,
                        );
                    }
                    Err(failure) => capture_delivery(
                        emit(&publish, id, AccountEventKind::SaveFailed(failure)),
                        &mut delivery_failure,
                    ),
                }
            }
            AccountCommand::Cancel(id)
                if pending_save
                    .as_ref()
                    .is_some_and(|(pending, _)| *pending == id) =>
            {
                file.discard_pending()
                    .map_err(|failure| AccountFailure::Storage(failure.to_string()))?;
                pending_save = None;
                capture_delivery(
                    emit(&publish, id, AccountEventKind::Cancelled),
                    &mut delivery_failure,
                );
            }
            AccountCommand::RetrySave(_) | AccountCommand::Cancel(_) => {}
        }
        Ok((
            Self {
                file,
                pending_save,
                credentials: owner_credentials,
                source,
                source_snapshot,
            },
            delivery_failure,
        ))
    }
    pub async fn finish(self) -> Result<(), AccountFailure> {
        let file = self.file;
        if file.has_pending_session_update() || file.has_pending_sign_out() {
            let (_, result) = blocking(file, AuthFile::save_pending).await?;
            result.map_err(|failure| AccountFailure::Storage(failure.to_string()))?;
        }
        Ok(())
    }
}

async fn read_source(
    source: &SourceSettings,
    credentials: &CredentialClient,
    previous: &mut Option<AccountSource>,
    publish: &impl Fn(AccountEvent) -> Result<(), DeliveryFailure>,
    delivery_failure: &mut Option<AccountFailure>,
) -> Result<(), AccountFailure> {
    let source = source.clone();
    let snapshot = tokio::task::spawn_blocking(move || source.read())
        .await
        .map_err(|_| AccountFailure::Task)?;
    update_source_snapshot(snapshot, credentials, previous, publish, delivery_failure)
}

fn update_source_snapshot(
    snapshot: AccountSource,
    credentials: &CredentialClient,
    previous: &mut Option<AccountSource>,
    publish: &impl Fn(AccountEvent) -> Result<(), DeliveryFailure>,
    delivery_failure: &mut Option<AccountFailure>,
) -> Result<(), AccountFailure> {
    if previous
        .as_ref()
        .is_some_and(|old| old.codex_identity() != snapshot.codex_identity())
    {
        credentials.update_source(snapshot.path().is_some(), true)?;
    }
    if previous.as_ref() != Some(&snapshot) {
        *previous = Some(snapshot.clone());
        capture_delivery(
            emit(publish, 0, AccountEventKind::SourceChanged(Ok(snapshot))),
            delivery_failure,
        );
    }
    Ok(())
}

fn emit(
    publish: &impl Fn(AccountEvent) -> Result<(), DeliveryFailure>,
    id: u64,
    kind: AccountEventKind,
) -> Result<(), AccountFailure> {
    publish(AccountEvent {
        operation_id: id,
        kind,
    })
    .map_err(AccountFailure::Delivery)
}

fn capture_delivery(result: Result<(), AccountFailure>, failure: &mut Option<AccountFailure>) {
    if let Err(error) = result {
        error!(error = %error, "认证状态无法交付，继续完成必要的凭据提交");
        if failure.is_none() {
            *failure = Some(error);
        }
    }
}

async fn sign_out(
    mut file: AuthFile,
    id: u64,
    credentials: &CredentialClient,
    publish: &impl Fn(AccountEvent) -> Result<(), DeliveryFailure>,
) -> Result<(AuthFile, Option<AccountFailure>), AccountFailure> {
    let mut delivery_failure = None;
    capture_delivery(
        emit(
            publish,
            id,
            AccountEventKind::SignOutProgress(SignOutStage::Settling),
        ),
        &mut delivery_failure,
    );
    debug!(
        operation_id = id,
        "退出已取得认证所有权，当前凭据更新已收敛"
    );
    let (returned, preparation) = blocking(file, AuthFile::prepare_sign_out).await?;
    file = returned;
    let prepared = match preparation {
        Ok(SignOutPreparation::Revoke(session)) => {
            capture_delivery(
                emit(
                    publish,
                    id,
                    AccountEventKind::SignOutProgress(SignOutStage::Revoking),
                ),
                &mut delivery_failure,
            );
            file.stage_sign_out(session.revoke().await)
        }
        Ok(_) => Ok(()),
        Err(failure) => Err(failure),
    };
    let result = match prepared {
        Ok(()) if file.has_pending_sign_out() => {
            capture_delivery(
                emit(
                    publish,
                    id,
                    AccountEventKind::SignOutProgress(SignOutStage::Clearing),
                ),
                &mut delivery_failure,
            );
            let (returned, saved) = blocking(file, AuthFile::save_pending).await?;
            file = returned;
            saved.map(|_| ()).map_err(SignOutFailure::local_clear)
        }
        other => other,
    };
    match result {
        Ok(()) => {
            let remote = file
                .sign_out_result()
                .ok_or_else(|| AccountFailure::Storage("退出缺少真实清理终态".into()))?;
            let snapshot = file
                .session_snapshot()
                .map_err(|failure| AccountFailure::Storage(failure.to_string()))?;
            credentials.finish_sign_out()?;
            if let RemoteRevocation::Unconfirmed(reason) = &remote {
                tracing::warn!(operation_id = id, reason, "本地会话已清除，远端撤销未确认");
            }
            debug!(operation_id = id, "退出登录的本地凭据清理已提交");
            capture_delivery(
                emit(
                    publish,
                    id,
                    AccountEventKind::SignedOut { snapshot, remote },
                ),
                &mut delivery_failure,
            );
        }
        Err(failure) => {
            error!(operation_id = id, error = %failure, "退出尚未完成，保持禁止新请求");
            capture_delivery(
                emit(publish, id, AccountEventKind::SignOutFailed(failure)),
                &mut delivery_failure,
            );
        }
    }
    Ok((file, delivery_failure))
}

async fn blocking<T: Send + 'static>(
    mut file: AuthFile,
    operation: impl FnOnce(&mut AuthFile) -> T + Send + 'static,
) -> Result<(AuthFile, T), AccountFailure> {
    tokio::task::spawn_blocking(move || {
        let result = operation(&mut file);
        (file, result)
    })
    .await
    .map_err(|_| AccountFailure::Task)
}

async fn login(
    mut file: AuthFile,
    id: u64,
    consent: bool,
    cancel: &CancellationToken,
    publish: &impl Fn(AccountEvent) -> Result<(), DeliveryFailure>,
) -> Result<
    (
        AuthFile,
        Option<(u64, CancellationToken)>,
        Option<AccountFailure>,
    ),
    AccountFailure,
> {
    if cancel.is_cancelled() {
        let delivery_failure = emit(publish, id, AccountEventKind::Cancelled).err();
        return Ok((file, None, delivery_failure));
    }
    let (returned, preparation) = blocking(file, AuthFile::prepare_login).await?;
    file = returned;
    let outcome: Result<(), LoginRunFailure> = async {
        let preparation = preparation?;
        if cancel.is_cancelled() {
            return Err(LoginFailure::cancelled().into());
        }
        let mut attempt = BrowserLogin::start(preparation, consent).await?;
        if cancel.is_cancelled() {
            return Err(LoginFailure::cancelled().into());
        }
        emit(
            publish,
            id,
            AccountEventKind::BrowserReady(attempt.authorization_url().into()),
        )
        .map_err(LoginRunFailure::Runtime)?;
        let issued = attempt.wait_for_callback(cancel).await?;
        file.remember_issued_client(issued);
        emit(publish, id, AccountEventKind::Exchanging).map_err(LoginRunFailure::Runtime)?;
        let verified = attempt.exchange(cancel).await?;
        if cancel.is_cancelled() {
            return Err(LoginFailure::cancelled().into());
        }
        file.stage_login(verified)
            .map_err(LoginRunFailure::Operation)
    }
    .await;
    match outcome {
        Ok(()) => {
            let mut delivery_failure = emit(publish, id, AccountEventKind::Saving).err();
            let (returned, result) = blocking(file, AuthFile::save_pending).await?;
            file = returned;
            match result {
                Ok(snapshot) => {
                    capture_delivery(
                        emit(publish, id, AccountEventKind::Completed(Ok(snapshot))),
                        &mut delivery_failure,
                    );
                    Ok((file, None, delivery_failure))
                }
                Err(failure) => {
                    capture_delivery(
                        emit(publish, id, AccountEventKind::SaveFailed(failure)),
                        &mut delivery_failure,
                    );
                    Ok((file, Some((id, cancel.clone())), delivery_failure))
                }
            }
        }
        Err(LoginRunFailure::Operation(failure)) if failure.stage == LoginStage::Cancelled => {
            let delivery_failure = emit(publish, id, AccountEventKind::Cancelled).err();
            Ok((file, None, delivery_failure))
        }
        Err(LoginRunFailure::Operation(failure)) => {
            let delivery_failure = emit(
                publish,
                id,
                AccountEventKind::Completed(Err(OperationFailure::Login(failure))),
            )
            .err();
            Ok((file, None, delivery_failure))
        }
        Err(LoginRunFailure::Runtime(failure)) => Ok((file, None, Some(failure))),
    }
}

enum LoginRunFailure {
    Operation(LoginFailure),
    Runtime(AccountFailure),
}

fn session_status(
    file: &AuthFile,
    activity: SessionActivity,
    failure: Option<SessionFailure>,
    publish: &impl Fn(AccountEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), AccountFailure> {
    let snapshot = file
        .session_snapshot()
        .map_err(|failure| AccountFailure::Storage(failure.to_string()))?;
    emit(
        publish,
        0,
        AccountEventKind::SessionStatus {
            snapshot,
            activity,
            failure,
        },
    )
}

async fn acquire(
    mut file: AuthFile,
    intent: CredentialIntent,
    publish: &impl Fn(AccountEvent) -> Result<(), DeliveryFailure>,
) -> Result<
    (
        AuthFile,
        Result<UsableCredentials, SessionFailure>,
        Option<AccountFailure>,
    ),
    AccountFailure,
> {
    if let Err(failure) = session_status(&file, SessionActivity::Checking, None, publish) {
        return Ok((
            file,
            Err(SessionFailure::new(
                SessionFailureKind::Runtime,
                "无法发布会话检查状态",
            )),
            Some(failure),
        ));
    }
    let rejected = match &intent {
        CredentialIntent::Acquire => None,
        CredentialIntent::Recover(version) => Some(version),
    };
    let mut delivery_failure = None;
    let result = match file.prepare_credentials(rejected) {
        Ok(CredentialPreparation::Available(credentials)) => Ok(credentials),
        Ok(preparation) => {
            let mut refresh_failure = None;
            if let CredentialPreparation::Refresh(attempt) = preparation {
                if let Err(failure) =
                    session_status(&file, SessionActivity::Refreshing, None, publish)
                {
                    return Ok((
                        file,
                        Err(SessionFailure::new(
                            SessionFailureKind::Runtime,
                            "无法发布会话续期状态",
                        )),
                        Some(failure),
                    ));
                }
                debug!("开始按需续期当前账号会话");
                match attempt.execute().await {
                    Ok(update) => {
                        if let Err(failure) = file.stage_refresh(update) {
                            file.record_refresh_failure(failure.clone());
                            refresh_failure = Some(failure);
                        }
                    }
                    Err(failure) => {
                        file.record_refresh_failure(failure.clone());
                        refresh_failure = Some(failure);
                    }
                }
            }
            if file.has_pending_session_update() {
                delivery_failure =
                    session_status(&file, SessionActivity::Saving, None, publish).err();
                let (returned, saved) = blocking(file, AuthFile::save_pending).await?;
                file = returned;
                match saved {
                    Ok(_) => match refresh_failure {
                        Some(failure) => Err(failure),
                        None => file.usable_credentials(),
                    },
                    Err(failure) => Err(SessionFailure::new(
                        SessionFailureKind::Storage,
                        format!("凭据更新尚未保存，请重试保存：{failure}"),
                    )),
                }
            } else {
                match refresh_failure {
                    Some(failure) => Err(failure),
                    None => Err(SessionFailure::new(
                        SessionFailureKind::Busy,
                        "凭据更新没有可提交结果",
                    )),
                }
            }
        }
        Err(failure) => Err(failure),
    };
    if let Err(failure) = &result {
        error!(kind = ?failure.kind, error = %failure, "当前请求未取得可用凭据");
    } else {
        debug!("当前请求已取得持久化的可用凭据");
    }
    let activity = if file.has_pending_session_update() {
        SessionActivity::SaveFailed
    } else {
        SessionActivity::Idle
    };
    let final_delivery = session_status(&file, activity, result.as_ref().err().cloned(), publish);
    if delivery_failure.is_none() {
        delivery_failure = final_delivery.err();
    }
    Ok((file, result, delivery_failure))
}
impl From<LoginFailure> for LoginRunFailure {
    fn from(failure: LoginFailure) -> Self {
        Self::Operation(failure)
    }
}
