use agentelier_account::{
    Account, AccountCommand, AccountEventKind, AccountFailure, CredentialClient,
};
use agentelier_conversation::SessionId;
use agentelier_conversation::{
    Conversation, ConversationEvent, ConversationFailure, ConversationRequest, MAX_PENDING_STEERS,
    RunnerEvent, SessionCommand, SteerInput,
};
use agentelier_llm_model::{ModelFailure, ModelRequest, ModelResult};
mod creation;
mod model_task;
use model_task::{
    ModelJoin, ModelTask, PROGRESS_CAPACITY, TaskRequest, complete, drain_progress, progress,
    stop_model,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::{error::Error, fmt, io};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

pub use agentelier_account::DeliveryFailure;

const COMMAND_CAPACITY: usize = 4;
const SIGN_OUT_CAPACITY: usize = 1;
const SESSION_COMMAND_CAPACITY: usize = 16;

pub struct AgentEvent {
    pub operation_id: u64,
    pub kind: AgentEventKind,
}
pub enum AgentEventKind {
    Account(AccountEventKind),
    ModelCompleted(Result<ModelResult, ModelFailure>),
    ModelCancelled,
    Conversation(ConversationEvent),
}
#[derive(Debug)]
pub enum RuntimeFailure {
    Startup(io::Error),
    Thread,
    ModelState,
    Account(AccountFailure),
    Conversation(ConversationFailure),
    Store(io::Error),
}
impl fmt::Display for RuntimeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Startup(error) => write!(f, "无法启动认证后台：{error}"),
            Self::Thread => f.write_str("认证运行线程异常结束"),
            Self::ModelState => f.write_str("模型终态缺少所属任务"),
            Self::Account(error) => error.fmt(f),
            Self::Conversation(error) => error.fmt(f),
            Self::Store(error) => write!(f, "会话存储失败：{error}"),
        }
    }
}
impl Error for RuntimeFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Startup(error) => Some(error),
            Self::Account(error) => Some(error),
            Self::Conversation(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Thread | Self::ModelState => None,
        }
    }
}
impl From<AccountFailure> for RuntimeFailure {
    fn from(failure: AccountFailure) -> Self {
        Self::Account(failure)
    }
}
impl From<ConversationFailure> for RuntimeFailure {
    fn from(failure: ConversationFailure) -> Self {
        Self::Conversation(failure)
    }
}
enum Command {
    Created(io::Result<agentelier_session_store::Commit>),
    Account(AccountCommand),
    Model {
        id: u64,
        control: ModelControl,
        request: ModelRequest,
    },
    ModelFinished(ModelJoin),
    Stream(Option<RunnerEvent>),
    Session(SessionCommand),
    Conversation {
        id: u64,
        control: ModelControl,
        request: ConversationRequest,
        steers: mpsc::Receiver<SteerInput>,
    },
}

pub struct AgentRuntime {
    commands: Option<mpsc::Sender<Command>>,
    thread: Option<JoinHandle<Result<(), RuntimeFailure>>>,
    active: Option<(u64, CancellationToken)>,
    credentials: CredentialClient,
    shutdown: CancellationToken,
    sign_out_commands: mpsc::Sender<u64>,
    model: SharedModel,
    session_commands: mpsc::Sender<SessionCommand>,
}

type SharedModel = Arc<Mutex<Option<ModelControl>>>;

#[derive(Clone)]
struct ModelControl {
    id: u64,
    turn: Arc<OnceLock<(SessionId, u64)>>,
    cancel: CancellationToken,
    finished: CancellationToken,
    steers: mpsc::Sender<SteerInput>,
}
impl ModelControl {
    fn new(id: u64) -> (Self, mpsc::Receiver<SteerInput>) {
        let (steers, incoming) = mpsc::channel(MAX_PENDING_STEERS);
        (
            Self {
                id,
                turn: Arc::new(OnceLock::new()),
                cancel: CancellationToken::new(),
                finished: CancellationToken::new(),
                steers,
            },
            incoming,
        )
    }
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
        let data_root = credentials_path
            .parent()
            .ok_or_else(|| RuntimeFailure::Store(io::Error::other("凭据路径缺少数据目录")))?
            .to_path_buf();
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
        let shared_model: SharedModel = Arc::new(Mutex::new(None));
        let owner_model = shared_model.clone();
        let (session_commands, mut session_incoming) = mpsc::channel(SESSION_COMMAND_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("agentelier-auth".into())
            .spawn(move || {
                debug!("认证运行线程已启动");
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async move {
                        let store = agentelier_session_store::SessionStore::start(data_root).map_err(RuntimeFailure::Store)?;
                        let mut creating: Option<creation::Creation> = None;
                        let mut account = Account::new(credentials_path, owner_credentials.clone());
                        let mut delivery_failure = None;
                        let mut model: Option<ModelTask> = None;
                        let mut conversation = Conversation::default();
                        let mut current_subject: Option<String> = None;
                        let mut progress_events: Option<mpsc::Receiver<RunnerEvent>> = None;
                        let execution = async {
                            loop {
                                let command = tokio::select! { biased;
                                    request = sign_out_incoming.recv() => match request {
                                        Some(id) => Command::Account(AccountCommand::SignOut(id)),
                                        None => break,
                                    },
                                    _ = runtime_shutdown.cancelled() => break,
                                    result = async { match &mut creating {
                                        Some(pending) => pending.wait().await,
                                        None => std::future::pending().await,
                                    }} => Command::Created(result),
                                    result = async { match &mut model {
                                        Some(task) => (&mut task.handle).await,
                                        None => std::future::pending().await,
                                    }} => Command::ModelFinished(result),
                                    command = session_incoming.recv() => match command {
                                        Some(command) => Command::Session(command),
                                        None => break,
                                    },
                                    event = async { match &mut progress_events {
                                        Some(receiver) => receiver.recv().await,
                                        None => std::future::pending().await,
                                    }} => Command::Stream(event),
                                    command = incoming.recv() => match command {
                                        Some(command) => command,
                                        None => break,
                                    },
                                };
                                match command {
                                    Command::Created(result) => {
                                        let pending = creating.take().ok_or(RuntimeFailure::ModelState)?;
                                        let id = pending.id;
                                        let event = pending.complete(result, current_subject.as_deref(), &mut conversation);
                                        emit(&publish, id, AgentEventKind::Conversation(event))?;
                                    }
                                    Command::Model {
                                        id,
                                        control,
                                        request,
                                    } => {
                                        if model.is_some() || creating.is_some() {
                                            control.finished.cancel();
                                            emit(&publish,id,AgentEventKind::ModelCompleted(Err(agentelier_llm_model::ModelFailure {kind:agentelier_llm_model::ModelFailureKind::Service,status:None,code:None,request_id:None,message:"当前任务正在执行，不能插入模型操作".into()})))?;
                                            continue;
                                        }
                                        *owner_model.lock().map_err(|_| RuntimeFailure::ModelState)?=Some(control.clone());
                                        model = Some(ModelTask::start(
                                            id,
                                            control,
                                            owner_credentials.clone(),
                                            store.client(),
                                            TaskRequest::Model(request),
                                        ));
                                    }
                                    Command::ModelFinished(result) => {
                                        let task =
                                            model.take().ok_or(RuntimeFailure::ModelState)?;
                                        task.control.finished.cancel();
                                        debug!(operation_id = task.id, "模型操作已真实结束并回收");
                                        capture_delivery(
                                            {
                                                drain_progress(
                                                    &mut conversation,
                                                    task.id,
                                                    &mut progress_events,
                                                    &publish,
                                                )?;
                                                emit(
                                                    &publish,
                                                    task.id,
                                                    complete(
                                                        &mut conversation,
                                                        task.id,
                                                        task.chat,
                                                        result,
                                                    )?,
                                                )
                                            },
                                            &mut delivery_failure,
                                        );
                                        if delivery_failure.is_none() && !runtime_shutdown.is_cancelled() {
                                            model_task::start_queued(&mut conversation,&mut model,&mut progress_events,&owner_model,&owner_credentials,&store,&publish)?;
                                        }
                                    }
                                    Command::Session(command) => {
                                        if creating.is_some() || (matches!(command,SessionCommand::Resume {..}) && model.is_some()) {
                                            let id=command.id();
                                            emit(&publish,id,AgentEventKind::Conversation(ConversationEvent::Queue(agentelier_conversation::QueueEvent {
                                                id,view:None,decision:agentelier_conversation::QueueDecision::Rejected("当前执行尚未回收，不能继续队列".into()),
                                            })))?;
                                            continue;
                                        }
                                        let event=conversation.queue_command(command);
                                        emit(&publish,event.id,AgentEventKind::Conversation(ConversationEvent::Queue(event)))?;
                                        if model.is_none() && !runtime_shutdown.is_cancelled() {
                                            model_task::start_queued(&mut conversation,&mut model,&mut progress_events,&owner_model,&owner_credentials,&store,&publish)?;
                                        }
                                    }
                                    Command::Stream(event) => {
                                        if let Some(event) = event {
                                            let task = model
                                                .as_ref()
                                                .filter(|task| task.chat)
                                                .ok_or(RuntimeFailure::ModelState)?;
                                            progress(&mut conversation, task.id, event, &publish)?;
                                        } else {
                                            progress_events = None;
                                        }
                                    }
                                    Command::Conversation {
                                        id,
                                        control,
                                        request,
                                        steers,
                                    } => {
                                        if model.is_some() || creating.is_some() {
                                            control.finished.cancel();
                                            emit(&publish,id,AgentEventKind::Conversation(ConversationEvent::Rejected("排队任务已经开始，当前发送未接纳，草稿保留".into())))?;
                                            continue;
                                        }
                                        match request {
                                            request @ (ConversationRequest::Create(_) | ConversationRequest::Reset { .. }) => {
                                                match creation::Creation::start(id, control.clone(), request, &conversation, current_subject.as_deref(), &store) {
                                                    Ok(pending) => {
                                                        *owner_model.lock().map_err(|_| RuntimeFailure::ModelState)? = Some(control);
                                                        creating = Some(pending);
                                                    }
                                                    Err(failure) => {
                                                        control.finished.cancel();
                                                        emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected(failure.to_string())))?;
                                                    }
                                                }
                                            }
                                            request @ (ConversationRequest::Send { .. } | ConversationRequest::Retry { .. }) => {
                                                match conversation.start(id, request) {
                                                    Ok((view, request)) => {
                                                        let turn = view.turn.as_ref().ok_or(RuntimeFailure::ModelState)?;
                                                        control.turn.set((view.id, turn.id.0)).map_err(|_| RuntimeFailure::ModelState)?;
                                                        *owner_model.lock().map_err(|_| RuntimeFailure::ModelState)?=Some(control.clone());
                                                        emit(
                                                            &publish,
                                                            id,
                                                            AgentEventKind::Conversation(
                                                                ConversationEvent::Accepted(view),
                                                            ),
                                                        )?;
                                                        let (sender, receiver) =
                                                            mpsc::channel(PROGRESS_CAPACITY);
                                                        progress_events = Some(receiver);
                                                        model = Some(ModelTask::start(
                                                            id,
                                                            control,
                                                            owner_credentials.clone(),
                                                            store.client(),
                                                            TaskRequest::Chat(Box::new(request), sender, steers),
                                                        ));
                                                    }
                                                    Err(failure) => {
                                                        control.finished.cancel();
                                                        emit(
                                                            &publish,
                                                            id,
                                                            AgentEventKind::Conversation(
                                                                ConversationEvent::Rejected(
                                                                    failure.to_string(),
                                                                ),
                                                            ),
                                                        )?;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Command::Account(command) => {
                                        let signing_out =
                                            matches!(&command, AccountCommand::SignOut(_));
                                        let acquiring =
                                            matches!(&command, AccountCommand::Credentials { .. });
                                        if signing_out {
                                            capture_delivery(
                                                stop_model(
                                                    &mut model,
                                                    &mut progress_events,
                                                    &mut conversation,
                                                    &publish,
                                                )
                                                .await,
                                                &mut delivery_failure,
                                            );
                                            while let Ok(queued) = incoming.try_recv() {
                                                match queued {
                                                    Command::Account(command) => {
                                                        command.reject_for_sign_out()
                                                    }
                                                    Command::Model { control, .. }
                                                    | Command::Conversation { control, .. } => {
                                                        control.finished.cancel()
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        let identity = std::cell::Cell::new(None);
                                        let (returned, failure) = account
                                            .handle(command, &|event| {
                                                match &event.kind {
                                                    AccountEventKind::Completed(Ok(snapshot))
                                                    | AccountEventKind::SessionStatus {
                                                        snapshot,
                                                        ..
                                                    }
                                                    | AccountEventKind::SignedOut {
                                                        snapshot,
                                                        ..
                                                    } => {
                                                        let subject = if matches!(
                                                            &snapshot.credentials,
                                                            agentelier_account::CredentialState::SignedOut
                                                                | agentelier_account::CredentialState::NoRecord
                                                                | agentelier_account::CredentialState::HostOnly
                                                        ) {
                                                            None
                                                        } else {
                                                            snapshot.account.as_ref().map(|account| account.subject.clone())
                                                        };
                                                        identity.set(Some(subject));
                                                    }
                                                    _ => {}
                                                }
                                                publish(AgentEvent {
                                                    operation_id: event.operation_id,
                                                    kind: AgentEventKind::Account(event.kind),
                                                })
                                            })
                                            .await?;
                                        account = returned;
                                        if let Some(subject) = identity.take() {
                                            conversation.synchronize_account(subject.as_deref());
                                            current_subject = subject;
                                        }
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
                            stop_model(
                                &mut model,
                                &mut progress_events,
                                &mut conversation,
                                &publish,
                            )
                            .await,
                            &mut delivery_failure,
                        );
                        let stored = store.finish().await.map_err(RuntimeFailure::Store);
                        let pending_saved = if let Some(mut pending) = creating {
                            let saved = pending.wait().await;
                            pending.control.finished.cancel();
                            saved.and_then(|commit| match commit.summary_failure {
                                Some(failure) => Err(failure),
                                None => Ok(()),
                            }).map_err(RuntimeFailure::Store)
                        } else {
                            Ok(())
                        };
                        let account_finished = match execution {
                            Ok(account) => account.finish().await.map_err(RuntimeFailure::Account),
                            Err(failure) => Err(failure),
                        };
                        if let Err(failure) = stored {
                            error!(error = %failure, "会话存储收尾失败");
                            account_finished?;
                            return Err(failure);
                        }
                        account_finished?;
                        pending_saved?;
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
            model: shared_model,
            session_commands,
        })
    }

    pub fn restore(&self, operation_id: u64) -> Result<(), RuntimeFailure> {
        self.send(Command::Account(AccountCommand::Restore(operation_id)))
    }
    pub fn credentials(&self) -> CredentialClient {
        self.credentials.clone()
    }
    pub fn request_model(&mut self, id: u64, request: ModelRequest) -> Result<(), RuntimeFailure> {
        let (control, steers) = ModelControl::new(id);
        drop(steers);
        self.send(Command::Model {
            id,
            request,
            control: control.clone(),
        })?;
        Ok(())
    }
    pub fn request_conversation(
        &mut self,
        id: u64,
        request: ConversationRequest,
    ) -> Result<(), RuntimeFailure> {
        let (control, steers) = ModelControl::new(id);
        self.send(Command::Conversation {
            steers,
            id,
            control: control.clone(),
            request,
        })?;
        Ok(())
    }
    fn active_model(&self) -> Result<Option<ModelControl>, RuntimeFailure> {
        self.model
            .lock()
            .map(|model| model.clone())
            .map_err(|_| RuntimeFailure::ModelState)
    }
    pub fn request_session(&self, command: SessionCommand) -> Result<(), RuntimeFailure> {
        if self.shutdown.is_cancelled() {
            return Err(AccountFailure::Closed.into());
        }
        self.session_commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AccountFailure::Busy,
                mpsc::error::TrySendError::Closed(_) => AccountFailure::Closed,
            })?;
        Ok(())
    }

    pub fn steer_conversation(&self, input: SteerInput) -> Result<(), RuntimeFailure> {
        let model = self.active_model()?;
        let control = model
            .as_ref()
            .filter(|control| {
                !self.shutdown.is_cancelled()
                    && !control.cancel.is_cancelled()
                    && !control.finished.is_cancelled()
                    && control.id == input.operation_id
                    && control.turn.get() == Some(&(input.session, input.turn.0))
            })
            .ok_or(AccountFailure::Busy)?;
        control
            .steers
            .try_send(input)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => AccountFailure::Busy,
                mpsc::error::TrySendError::Closed(_) => AccountFailure::Closed,
            })?;
        Ok(())
    }

    pub fn cancel_conversation(
        &self,
        session: SessionId,
        turn: u64,
        operation: u64,
    ) -> Result<bool, RuntimeFailure> {
        let model = self.active_model()?;
        let Some(control) = model.as_ref().filter(|control| {
            control.id == operation
                && control.turn.get() == Some(&(session, turn))
                && !control.finished.is_cancelled()
        }) else {
            return Ok(false);
        };
        control.cancel.cancel();
        Ok(true)
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
        if let Some(model) = self.active_model()? {
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
            .active_model()?
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
        match self.active_model() {
            Ok(Some(model)) => model.cancel.cancel(),
            Ok(None) => {}
            Err(failure) => error!(error=%failure,"关闭时无法读取活动任务，运行线程仍将回收任务"),
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
