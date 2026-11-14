use agentelier_login::{
    AuthFile, BrowserLogin, LoginFailure, LoginStage, RestoreFailure, RestoredAccount,
};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::{error::Error, fmt, io};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

const COMMAND_CAPACITY: usize = 4;

pub struct AgentEvent {
    pub operation_id: u64,
    pub kind: AgentEventKind,
}
pub enum AgentEventKind {
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
}
impl fmt::Display for RuntimeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Startup(error) => write!(f, "无法启动认证后台：{error}"),
            Self::Busy => f.write_str("认证后台命令队列已满"),
            Self::Closed => f.write_str("认证后台已停止接收命令"),
            Self::Task => f.write_str("认证文件任务异常结束"),
            Self::Thread => f.write_str("认证运行线程异常结束"),
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

enum Command {
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
        let thread = std::thread::Builder::new()
            .name("agentelier-auth".into())
            .spawn(move || {
                debug!("认证运行线程已启动");
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async move {
                        let mut file = AuthFile::new(credentials_path);
                        let mut pending_save = None;
                        while let Some(command) = incoming.recv().await {
                            match command {
                                Command::Restore(id) => {
                                    debug!(operation_id = id, "开始恢复当前目录账号");
                                    let (returned, result) =
                                        blocking(file, AuthFile::restore).await?;
                                    file = returned;
                                    emit(
                                        &publish,
                                        id,
                                        AgentEventKind::Completed(
                                            result.map_err(OperationFailure::Restore),
                                        ),
                                    )?;
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
        })
    }

    pub fn restore(&self, operation_id: u64) -> Result<(), RuntimeFailure> {
        self.send(Command::Restore(operation_id))
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
impl From<LoginFailure> for LoginRunFailure {
    fn from(failure: LoginFailure) -> Self {
        Self::Operation(failure)
    }
}
