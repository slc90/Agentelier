use agentelier_account::{
    Account, AccountCommand, AccountEventKind, AccountFailure, CredentialClient,
};
use agentelier_llm_model::{ModelFailure, ModelFailureKind, ModelReply};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::{error::Error, fmt, io};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

pub use agentelier_account::DeliveryFailure;

const COMMAND_CAPACITY: usize = 4;
const SIGN_OUT_CAPACITY: usize = 1;

pub struct AgentEvent {
    pub operation_id: u64,
    pub kind: AgentEventKind,
}
pub enum AgentEventKind {
    Account(AccountEventKind),
    ModelCompleted(Result<ModelReply, ModelFailure>),
    ModelCancelled,
}
#[derive(Debug)]
pub enum RuntimeFailure {
    Startup(io::Error),
    Thread,
    ModelState,
    Account(AccountFailure),
}
impl fmt::Display for RuntimeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Startup(error) => write!(f, "无法启动认证后台：{error}"),
            Self::Thread => f.write_str("认证运行线程异常结束"),
            Self::ModelState => f.write_str("模型终态缺少所属任务"),
            Self::Account(error) => error.fmt(f),
        }
    }
}
impl Error for RuntimeFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Startup(error) => Some(error),
            Self::Account(error) => Some(error),
            Self::Thread | Self::ModelState => None,
        }
    }
}
impl From<AccountFailure> for RuntimeFailure {
    fn from(failure: AccountFailure) -> Self {
        Self::Account(failure)
    }
}
enum Command {
    Account(AccountCommand),
    VerifyModel { id: u64, control: ModelControl },
    ModelFinished(Result<Option<Result<ModelReply, ModelFailure>>, tokio::task::JoinError>),
}

pub struct AgentRuntime {
    commands: Option<mpsc::Sender<Command>>,
    thread: Option<JoinHandle<Result<(), RuntimeFailure>>>,
    active: Option<(u64, CancellationToken)>,
    credentials: CredentialClient,
    shutdown: CancellationToken,
    sign_out_commands: mpsc::Sender<u64>,
    model: Option<ModelControl>,
}

#[derive(Clone)]
struct ModelControl {
    cancel: CancellationToken,
    finished: CancellationToken,
}
struct ModelTask {
    id: u64,
    control: ModelControl,
    handle: tokio::task::JoinHandle<Option<Result<ModelReply, ModelFailure>>>,
}
impl ModelTask {
    fn start(id: u64, control: ModelControl, credentials: CredentialClient) -> Self {
        let cancel = control.cancel.clone();
        let handle = tokio::spawn(async move {
            let started = std::time::Instant::now();
            debug!(operation_id = id, "模型连接操作执行体已启动");
            let result = tokio::select! { biased;
                _ = cancel.cancelled() => None,
                result = agentelier_llm_model::verify(credentials) => Some(result),
            };
            debug!(
                operation_id = id,
                elapsed_ms = started.elapsed().as_millis(),
                cancelled = result.is_none(),
                "模型连接操作执行体已结束"
            );
            result
        });
        Self {
            id,
            control,
            handle,
        }
    }
}
fn model_event(
    result: Result<Option<Result<ModelReply, ModelFailure>>, tokio::task::JoinError>,
) -> AgentEventKind {
    match result {
        Ok(Some(result)) => AgentEventKind::ModelCompleted(result),
        Ok(None) => AgentEventKind::ModelCancelled,
        Err(_) => AgentEventKind::ModelCompleted(Err(ModelFailure {
            kind: ModelFailureKind::Service,
            status: None,
            code: None,
            request_id: None,
            message: "模型操作任务异常结束".into(),
        })),
    }
}
async fn stop_model(
    model: &mut Option<ModelTask>,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    if let Some(task) = model.take() {
        task.control.cancel.cancel();
        debug!(operation_id = task.id, "已请求取消模型操作，等待执行体结束");
        let result = task.handle.await;
        task.control.finished.cancel();
        debug!(operation_id = task.id, "模型操作已真实结束并回收");
        emit(publish, task.id, model_event(result))?;
    }
    Ok(())
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
        let credential_commands = commands.clone();
        let credentials = CredentialClient::new(shutdown.clone(), move |command| {
            credential_commands
                .try_send(Command::Account(command))
                .map_err(|failure| match failure {
                    mpsc::error::TrySendError::Full(_) => AccountFailure::Busy,
                    mpsc::error::TrySendError::Closed(_) => AccountFailure::Closed,
                })
        });
        let owner_credentials = credentials.clone();
        let (sign_out_commands, mut sign_out_incoming) = mpsc::channel(SIGN_OUT_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("agentelier-auth".into())
            .spawn(move || {
                debug!("认证运行线程已启动");
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async move {
                        let mut account = Account::new(credentials_path, owner_credentials.clone());
                        let mut delivery_failure = None;
                        let mut model: Option<ModelTask> = None;
                        let execution = async {
                            loop {
                                let command = tokio::select! { biased;
                                    request = sign_out_incoming.recv() => match request {
                                        Some(id) => Command::Account(AccountCommand::SignOut(id)),
                                        None => break,
                                    },
                                    _ = runtime_shutdown.cancelled() => break,
                                    result = async { match &mut model {
                                        Some(task) => (&mut task.handle).await,
                                        None => std::future::pending().await,
                                    }} => Command::ModelFinished(result),
                                    command = incoming.recv() => match command {
                                        Some(command) => command,
                                        None => break,
                                    },
                                };
                                match command {
                                    Command::VerifyModel { id, control } => {
                                        if model.is_some() {
                                            return Err(AccountFailure::Busy.into());
                                        }
                                        model = Some(ModelTask::start(
                                            id,
                                            control,
                                            owner_credentials.clone(),
                                        ));
                                    }
                                    Command::ModelFinished(result) => {
                                        let task =
                                            model.take().ok_or(RuntimeFailure::ModelState)?;
                                        task.control.finished.cancel();
                                        debug!(operation_id = task.id, "模型操作已真实结束并回收");
                                        capture_delivery(
                                            emit(&publish, task.id, model_event(result)),
                                            &mut delivery_failure,
                                        );
                                    }
                                    Command::Account(command) => {
                                        let signing_out =
                                            matches!(&command, AccountCommand::SignOut(_));
                                        let acquiring =
                                            matches!(&command, AccountCommand::Credentials { .. });
                                        if signing_out {
                                            capture_delivery(
                                                stop_model(&mut model, &publish).await,
                                                &mut delivery_failure,
                                            );
                                            while let Ok(queued) = incoming.try_recv() {
                                                match queued {
                                                    Command::Account(command) => {
                                                        command.reject_for_sign_out()
                                                    }
                                                    Command::VerifyModel { control, .. } => {
                                                        control.finished.cancel()
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        let (returned, failure) = account
                                            .handle(command, &|event| {
                                                publish(AgentEvent {
                                                    operation_id: event.operation_id,
                                                    kind: AgentEventKind::Account(event.kind),
                                                })
                                            })
                                            .await?;
                                        account = returned;
                                        if failure.is_some()
                                            && (acquiring || delivery_failure.is_none())
                                        {
                                            delivery_failure = failure.map(RuntimeFailure::Account);
                                        }
                                        if signing_out && delivery_failure.is_some() {
                                            break;
                                        }
                                    }
                                }
                                if delivery_failure.is_some()
                                    && !owner_credentials.is_signing_out()?
                                {
                                    break;
                                }
                            }
                            Ok::<Account, RuntimeFailure>(account)
                        }
                        .await;
                        capture_delivery(
                            stop_model(&mut model, &publish).await,
                            &mut delivery_failure,
                        );
                        execution?.finish().await?;
                        if let Some(failure) = delivery_failure {
                            return Err(failure);
                        }
                        Ok(())
                    })
                }))
                .unwrap_or(Err(RuntimeFailure::Thread));
                drop(runtime);
                debug!("认证执行体与 Tokio 已结束");
                let notification = finished()
                    .map_err(|failure| RuntimeFailure::Account(AccountFailure::Delivery(failure)));
                outcome.and(notification)
            })
            .map_err(RuntimeFailure::Startup)?;
        Ok(Self {
            commands: Some(commands),
            thread: Some(thread),
            active: None,
            credentials,
            shutdown,
            sign_out_commands,
            model: None,
        })
    }

    pub fn restore(&self, operation_id: u64) -> Result<(), RuntimeFailure> {
        self.send(Command::Account(AccountCommand::Restore(operation_id)))
    }
    pub fn credentials(&self) -> CredentialClient {
        self.credentials.clone()
    }
    pub fn verify_model(&mut self, id: u64) -> Result<(), RuntimeFailure> {
        let control = ModelControl {
            cancel: CancellationToken::new(),
            finished: CancellationToken::new(),
        };
        self.send(Command::VerifyModel {
            id,
            control: control.clone(),
        })?;
        self.model = Some(control);
        Ok(())
    }
    pub fn sign_out(&mut self, id: u64) -> Result<(), RuntimeFailure> {
        if self.shutdown.is_cancelled() {
            return Err(AccountFailure::Closed.into());
        }
        self.credentials.begin_sign_out(|| {
            self.sign_out_commands
                .try_send(id)
                .map_err(|failure| match failure {
                    mpsc::error::TrySendError::Full(_) => AccountFailure::Busy,
                    mpsc::error::TrySendError::Closed(_) => AccountFailure::Closed,
                })
        })?;
        if let Some(model) = &self.model {
            model.cancel.cancel();
        }
        if let Some((_, cancel)) = &self.active {
            cancel.cancel();
        }
        Ok(())
    }
    pub fn login(&mut self, id: u64, consent: bool) -> Result<(), RuntimeFailure> {
        let cancel = CancellationToken::new();
        self.send(Command::Account(AccountCommand::Login {
            id,
            consent,
            cancel: cancel.clone(),
        }))?;
        self.active = Some((id, cancel));
        Ok(())
    }
    pub fn retry_save(&self, operation_id: u64) -> Result<(), RuntimeFailure> {
        self.send(Command::Account(AccountCommand::RetrySave(operation_id)))
    }
    pub fn cancel(&self, operation_id: u64) -> Result<(), RuntimeFailure> {
        if let Some((id, token)) = &self.active
            && *id == operation_id
        {
            token.cancel();
        }
        self.send(Command::Account(AccountCommand::Cancel(operation_id)))
    }
    fn send(&self, command: Command) -> Result<(), RuntimeFailure> {
        if self
            .model
            .as_ref()
            .is_some_and(|model| !model.finished.is_cancelled())
        {
            return Err(AccountFailure::Busy.into());
        }
        self.credentials
            .submit_operation(|| {
                self.commands
                    .as_ref()
                    .ok_or(AccountFailure::Closed)?
                    .try_send(command)
                    .map_err(|error| match error {
                        mpsc::error::TrySendError::Full(_) => AccountFailure::Busy,
                        mpsc::error::TrySendError::Closed(_) => AccountFailure::Closed,
                    })
            })
            .map_err(Into::into)
    }
    pub fn begin_shutdown(&mut self) {
        self.shutdown.cancel();
        if let Some(model) = &self.model {
            model.cancel.cancel();
        }
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
    .map_err(|failure| RuntimeFailure::Account(AccountFailure::Delivery(failure)))
}

fn capture_delivery(result: Result<(), RuntimeFailure>, failure: &mut Option<RuntimeFailure>) {
    if let Err(error) = result {
        error!(error = %error, "认证状态无法交付，继续完成必要的凭据提交");
        if failure.is_none() {
            *failure = Some(error);
        }
    }
}
