use crate::credentials::{CredentialClient, CredentialIntent};
use agentelier_login::{
    AuthFile, BrowserLogin, CredentialPreparation, LoginFailure, LoginStage, RestoreFailure,
    RestoredAccount, SessionActivity, SessionFailure, SessionFailureKind, UsableCredentials,
};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::{error::Error, fmt, io};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

const COMMAND_CAPACITY: usize = 4;

pub struct AgentEvent {
    pub operation_id: u64,
    pub kind: AgentEventKind,
}
pub enum AgentEventKind {
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
pub enum RuntimeFailure {
    Startup(io::Error),
    Busy,
    Closed,
    Task,
    Thread,
    Delivery(DeliveryFailure),
    CredentialState,
    Storage(String),
}
impl fmt::Display for RuntimeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Startup(error) => write!(f, "无法启动认证后台：{error}"),
            Self::Busy => f.write_str("认证后台命令队列已满"),
            Self::Closed => f.write_str("认证后台已停止接收命令"),
            Self::Task => f.write_str("认证文件任务异常结束"),
            Self::Thread => f.write_str("认证运行线程异常结束"),
            Self::CredentialState => f.write_str("凭据共享请求状态锁已损坏"),
            Self::Storage(message) => f.write_str(message),
            Self::Delivery(DeliveryFailure::Closed) => f.write_str("认证结果接收端已关闭"),
            Self::Delivery(DeliveryFailure::Full) => f.write_str("认证结果队列已满"),
            Self::Delivery(DeliveryFailure::Wake) => {
                f.write_str("认证结果已入队，但无法唤醒应用窗口")
            }
        }
    }
}
impl Error for RuntimeFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Startup(error) => Some(error),
            _ => None,
        }
    }
}

pub(super) enum Command {
    Credentials {
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

pub struct AgentRuntime {
    commands: Option<mpsc::Sender<Command>>,
    thread: Option<JoinHandle<Result<(), RuntimeFailure>>>,
    active: Option<(u64, CancellationToken)>,
    credentials: CredentialClient,
    shutdown: CancellationToken,
}

impl AgentRuntime {
    pub fn start(
        credentials_path: PathBuf,
        publish: impl Fn(AgentEvent) -> Result<(), DeliveryFailure> + Send + 'static,
        finished: impl Fn() -> Result<(), DeliveryFailure> + Send + 'static,
    ) -> Result<Self, RuntimeFailure> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(RuntimeFailure::Startup)?;
        let (commands, mut incoming) = mpsc::channel(COMMAND_CAPACITY);
        let shutdown = CancellationToken::new();
        let runtime_shutdown = shutdown.clone();
        let credentials = CredentialClient::new(commands.clone(), shutdown.clone());
        let thread = std::thread::Builder::new()
            .name("agentelier-auth".into())
            .spawn(move || {
                debug!("认证运行线程已启动");
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async move {
                        let mut file = AuthFile::new(credentials_path);
                        let mut pending_save = None;
                        let mut delivery_failure = None;
                        loop {
                            let command = tokio::select! { biased;
                                _ = runtime_shutdown.cancelled() => break,
                                command = incoming.recv() => match command { Some(command) => command, None => break },
                            };
                            match command {
                                Command::Credentials { intent, result } => {
                                    let (returned, outcome, failure) = acquire(file, intent, &publish).await?;
                                    file = returned;
                                    result.send_replace(Some(outcome));
                                    if failure.is_some() {
                                        delivery_failure = failure;
                                        break;
                                    }
                                }
                                Command::Restore(id) => {
                                    debug!(operation_id = id, "开始恢复当前目录账号");
                                    let (returned, result) =
                                        blocking(file, AuthFile::restore).await?;
                                    file = returned;
                                    let restored = result.is_ok();
                                    emit(
                                        &publish,
                                        id,
                                        AgentEventKind::Completed(
                                            result.map_err(OperationFailure::Restore),
                                        ),
                                    )?;
                                    if restored {
                                        session_status(&file, SessionActivity::Idle, file.session_failure(), &publish)?;
                                    }
                                }
                                Command::Login {
                                    id,
                                    consent,
                                    cancel,
                                } => {
                                    let (returned, pending) =
                                        login(file, id, consent, &cancel, &publish).await?;
                                    file = returned;
                                    pending_save = pending;
                                }
                                Command::RetrySave(id)
                                    if pending_save
                                        .as_ref()
                                        .is_some_and(|(pending, _)| *pending == id) =>
                                {
                                    if pending_save.as_ref().is_some_and(
                                        |(_, token): &(u64, CancellationToken)| {
                                            token.is_cancelled()
                                        },
                                    ) {
                                        file.discard_pending();
                                        pending_save = None;
                                        emit(&publish, id, AgentEventKind::Cancelled)?;
                                        continue;
                                    }
                                    emit(&publish, id, AgentEventKind::Saving)?;
                                    let (returned, result) =
                                        blocking(file, AuthFile::save_pending).await?;
                                    file = returned;
                                    match result {
                                        Ok(snapshot) => {
                                            pending_save = None;
                                            emit(
                                                &publish,
                                                id,
                                                AgentEventKind::Completed(Ok(snapshot)),
                                            )?;
                                        }
                                        Err(failure) => {
                                            emit(&publish, id, AgentEventKind::SaveFailed(failure))?
                                        }
                                    }
                                }
                                Command::Cancel(id)
                                    if pending_save
                                        .as_ref()
                                        .is_some_and(|(pending, _)| *pending == id) =>
                                {
                                    file.discard_pending();
                                    pending_save = None;
                                    emit(&publish, id, AgentEventKind::Cancelled)?;
                                }
                                Command::RetrySave(_) | Command::Cancel(_) => {}
                            }
                        }
                        if file.has_pending_session_update() {
                            let (_, result) = blocking(file, AuthFile::save_pending).await?;
                            result.map_err(|failure| RuntimeFailure::Storage(failure.to_string()))?;
                        }
                        if let Some(failure) = delivery_failure { return Err(failure); }
                        Ok(())
                    })
                }))
                .unwrap_or(Err(RuntimeFailure::Thread));
                drop(runtime);
                debug!("认证执行体与 Tokio 已结束");
                let notification = finished().map_err(RuntimeFailure::Delivery);
                outcome.and(notification)
            })
            .map_err(RuntimeFailure::Startup)?;
        Ok(Self {
            commands: Some(commands),
            thread: Some(thread),
            active: None,
            credentials,
            shutdown,
        })
    }

    pub fn restore(&self, operation_id: u64) -> Result<(), RuntimeFailure> {
        self.send(Command::Restore(operation_id))
    }
    pub fn credentials(&self) -> CredentialClient {
        self.credentials.clone()
    }
    pub fn login(&mut self, id: u64, consent: bool) -> Result<(), RuntimeFailure> {
        let cancel = CancellationToken::new();
        self.send(Command::Login {
            id,
            consent,
            cancel: cancel.clone(),
        })?;
        self.active = Some((id, cancel));
        Ok(())
    }
    pub fn retry_save(&self, operation_id: u64) -> Result<(), RuntimeFailure> {
        self.send(Command::RetrySave(operation_id))
    }
    pub fn cancel(&self, operation_id: u64) -> Result<(), RuntimeFailure> {
        if let Some((id, token)) = &self.active
            && *id == operation_id
        {
            token.cancel();
        }
        self.send(Command::Cancel(operation_id))
    }
    fn send(&self, command: Command) -> Result<(), RuntimeFailure> {
        self.commands
            .as_ref()
            .ok_or(RuntimeFailure::Closed)?
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => RuntimeFailure::Busy,
                mpsc::error::TrySendError::Closed(_) => RuntimeFailure::Closed,
            })
    }
    pub fn begin_shutdown(&mut self) {
        self.shutdown.cancel();
        if let Some((_, token)) = &self.active {
            token.cancel();
        }
        self.commands = None;
    }
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
    pub fn finish(&mut self) -> Result<(), RuntimeFailure> {
        self.begin_shutdown();
        match self.thread.take() {
            Some(thread) => thread.join().map_err(|_| RuntimeFailure::Thread)?,
            None => Ok(()),
        }
    }
}
impl Drop for AgentRuntime {
    fn drop(&mut self) {
        if let Err(failure) = self.finish() {
            error!(error = %failure, "未显式收尾的认证运行在释放时结束失败");
        }
    }
}

fn emit(
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    id: u64,
    kind: AgentEventKind,
) -> Result<(), RuntimeFailure> {
    publish(AgentEvent {
        operation_id: id,
        kind,
    })
    .map_err(RuntimeFailure::Delivery)
}

async fn blocking<T: Send + 'static>(
    mut file: AuthFile,
    operation: impl FnOnce(&mut AuthFile) -> T + Send + 'static,
) -> Result<(AuthFile, T), RuntimeFailure> {
    tokio::task::spawn_blocking(move || {
        let result = operation(&mut file);
        (file, result)
    })
    .await
    .map_err(|_| RuntimeFailure::Task)
}

async fn login(
    mut file: AuthFile,
    id: u64,
    consent: bool,
    cancel: &CancellationToken,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(AuthFile, Option<(u64, CancellationToken)>), RuntimeFailure> {
    if cancel.is_cancelled() {
        emit(publish, id, AgentEventKind::Cancelled)?;
        return Ok((file, None));
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
            AgentEventKind::BrowserReady(attempt.authorization_url().into()),
        )
        .map_err(LoginRunFailure::Runtime)?;
        let issued = attempt.wait_for_callback(cancel).await?;
        file.remember_issued_client(issued);
        emit(publish, id, AgentEventKind::Exchanging).map_err(LoginRunFailure::Runtime)?;
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
            emit(publish, id, AgentEventKind::Saving)?;
            let (returned, result) = blocking(file, AuthFile::save_pending).await?;
            file = returned;
            match result {
                Ok(snapshot) => {
                    emit(publish, id, AgentEventKind::Completed(Ok(snapshot)))?;
                    Ok((file, None))
                }
                Err(failure) => {
                    emit(publish, id, AgentEventKind::SaveFailed(failure))?;
                    Ok((file, Some((id, cancel.clone()))))
                }
            }
        }
        Err(LoginRunFailure::Operation(failure)) if failure.stage == LoginStage::Cancelled => {
            emit(publish, id, AgentEventKind::Cancelled)?;
            Ok((file, None))
        }
        Err(LoginRunFailure::Operation(failure)) => {
            emit(
                publish,
                id,
                AgentEventKind::Completed(Err(OperationFailure::Login(failure))),
            )?;
            Ok((file, None))
        }
        Err(LoginRunFailure::Runtime(failure)) => Err(failure),
    }
}

enum LoginRunFailure {
    Operation(LoginFailure),
    Runtime(RuntimeFailure),
}

fn session_status(
    file: &AuthFile,
    activity: SessionActivity,
    failure: Option<SessionFailure>,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    let snapshot = file
        .session_snapshot()
        .map_err(|failure| RuntimeFailure::Storage(failure.to_string()))?;
    emit(
        publish,
        0,
        AgentEventKind::SessionStatus {
            snapshot,
            activity,
            failure,
        },
    )
}

async fn acquire(
    mut file: AuthFile,
    intent: CredentialIntent,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<
    (
        AuthFile,
        Result<UsableCredentials, SessionFailure>,
        Option<RuntimeFailure>,
    ),
    RuntimeFailure,
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
