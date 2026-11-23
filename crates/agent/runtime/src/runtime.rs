use agentelier_account::{
    Account, AccountCommand, AccountEffect, AccountEvent, AccountEventKind, AccountFailure,
    CredentialClient, LoginPrompt, LogoutCompletion, RefreshCompletion,
};
use agentelier_conversation::{
    AgentRuntimeId, AudioId, Conversation, ConversationEvent, ConversationFailure,
    ConversationRequest, MAX_PENDING_STEERS, SessionCommand, SessionId, SteerInput,
};
use agentelier_llm_model::{ModelFailure, ModelRequest, ModelResult};
use agentelier_memory::Memory;
use agentelier_turn_runner::RunnerEvent;
mod account_login;
mod audio_task;
mod background_sessions;
mod child_runtime;
mod creation;
mod deleting;
mod image_attachment;
pub use image_attachment::{ImageAttachmentAction, ImageAttachmentReply};
mod interrupt_agent;
mod list_agents;
mod loading;
mod mailbox;
mod memory;
pub use memory::{MemoryReply, MemoryRequest};
mod model_task;
mod recording;
mod renaming;
mod session_model;
mod skill_creation;
mod skill_resource;
mod spawn_agent;
mod task_control;
mod task_projection;
mod tool_jobs;
mod wait_agent;
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
    McpConfig(io::Result<agentelier_mcp::ConfigSnapshot>),
    McpStatus(Vec<agentelier_mcp::ConnectionStatus>),
    McpMaterial(Result<agentelier_mcp::MaterialReply, String>),
    Memory(Result<MemoryReply, String>),
    ImageAttachment(Result<ImageAttachmentReply, String>),
}
#[derive(Debug)]
pub enum RuntimeFailure {
    Startup(io::Error),
    Thread,
    ModelState,
    Account(AccountFailure),
    Conversation(ConversationFailure),
    Store(io::Error),
    Memory(io::Error),
    Mcp(io::Error),
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
            Self::Memory(error) => write!(f, "记忆存储收尾失败：{error}"),
            Self::Mcp(error) => write!(f, "MCP 连接收尾失败：{error}"),
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
            Self::Memory(error) => Some(error),
            Self::Mcp(error) => Some(error),
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
struct PendingMaterial {
    id: u64,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Result<agentelier_mcp::MaterialReply, String>>,
}
enum SourceSettlement {
    Publish(AccountEvent),
    Logout(u64),
}

enum Command {
    McpMaterial(u64, agentelier_mcp::MaterialRequest),
    CancelMcpMaterial(u64),
    Memory(u64, MemoryRequest),
    ImageAttachment(u64, SessionId, String, ImageAttachmentAction),
    ImageAttachmentFinished(u64, io::Result<ImageAttachmentReply>),
    AudioDraft(u64, SessionId, Option<PathBuf>),
    TranscribeAudio(u64, SessionId, AudioId),
    CancelTranscription(u64, SessionId),
    AudioFinished(u64, SessionId, audio_task::Reply),
    McpMaterialFinished(Result<agentelier_mcp::MaterialReply, String>),
    McpConfig(u64, agentelier_mcp::ConfigChange),
    McpStatus(Vec<agentelier_mcp::ConnectionStatus>),
    Rename {
        id: u64,
        session: SessionId,
        title: String,
    },
    Renamed(io::Result<agentelier_session_store::RenameCommit>),
    Scan,
    Scanned(io::Result<agentelier_session_store::ArchiveCatalog>),
    ChangeProject(u64, agentelier_conversation::ProjectChange),
    ChangeSessionModel(session_model::Change),
    ProjectSaved(u64, io::Result<()>),
    Deleted(io::Result<agentelier_session_store::DeleteCommit>),
    Loaded(Box<io::Result<agentelier_conversation::RestoredSession>>),
    Recorded(
        Result<
            (
                recording::Pending,
                io::Result<agentelier_session_store::Commit>,
            ),
            tokio::task::JoinError,
        >,
    ),
    Created(io::Result<agentelier_session_store::Commit>),
    Account(AccountCommand),
    RefreshQuota,
    QuotaFinished(Result<agentelier_account::QuotaCompletion, tokio::task::JoinError>),
    BeginLogin(u64),
    CancelLogin(u64),
    LoginWaiting(u64, LoginPrompt),
    Model {
        id: u64,
        control: ModelControl,
        request: ModelRequest,
    },
    ModelFinished(ModelJoin),
    SpawnAgent(Box<spawn_agent::Invocation>),
    Mailbox(mailbox::Command),
    ChildFinished(Result<child_runtime::FinishedChild, tokio::task::JoinError>),
    RootStopping,
    Stream(Option<RunnerEvent>),
    Session(SessionCommand),
    TaskAction {
        id: u64,
        session: SessionId,
        action: agentelier_conversation::TaskAction,
    },
    TaskActionFinished(u64, agentelier_conversation::TaskActionReceipt),
    ProcessesChanged,
    IdleTool(agentelier_turn_runner::ToolEvent),
    BackgroundSession(SessionId, background_sessions::Signal),
    Conversation {
        id: u64,
        control: ModelControl,
        request: ConversationRequest,
        steers: mpsc::Receiver<SteerInput>,
        task_actions: mpsc::Receiver<agentelier_turn_runner::SessionTaskCommand>,
    },
}

pub struct AgentRuntime {
    commands: Option<mpsc::Sender<Command>>,
    thread: Option<JoinHandle<Result<(), RuntimeFailure>>>,
    credentials: CredentialClient,
    shutdown: CancellationToken,
    model: SharedModel,
    session_commands: mpsc::Sender<SessionCommand>,
}

type SharedModel = Arc<Mutex<Option<ModelControl>>>;

struct RootTurn {
    session: SessionId,
    turn: u64,
    instance: AgentRuntimeId,
}

#[derive(Clone)]
struct ModelControl {
    id: u64,
    turn: Arc<OnceLock<RootTurn>>,
    cancel: CancellationToken,
    finished: CancellationToken,
    steers: mpsc::Sender<SteerInput>,
    task_actions: mpsc::Sender<agentelier_turn_runner::SessionTaskCommand>,
}
impl ModelControl {
    fn bind_root(
        &self,
        conversation: &mut Conversation,
        view: &agentelier_conversation::SessionView,
    ) -> Result<(), RuntimeFailure> {
        let turn = view.turn.as_ref().ok_or(RuntimeFailure::ModelState)?.id.0;
        let session = view.id;
        let instance = conversation
            .root_instance(session)
            .ok_or(RuntimeFailure::ModelState)?;
        self.turn
            .set(RootTurn {
                session,
                turn,
                instance,
            })
            .map_err(|_| RuntimeFailure::ModelState)?;
        conversation
            .sync_root_state(view)
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        debug!(session_id = %session, runtime_instance = %instance, turn_id = turn, "Root Agent 开始执行");
        Ok(())
    }

    fn matches_turn(&self, session: SessionId, turn: u64) -> bool {
        self.turn
            .get()
            .is_some_and(|bound| bound.session == session && bound.turn == turn)
    }

    fn new(
        id: u64,
    ) -> (
        Self,
        mpsc::Receiver<SteerInput>,
        mpsc::Receiver<agentelier_turn_runner::SessionTaskCommand>,
    ) {
        let (steers, incoming) = mpsc::channel(MAX_PENDING_STEERS);
        let (task_actions, tasks) = mpsc::channel(MAX_PENDING_STEERS);
        (
            Self {
                id,
                turn: Arc::new(OnceLock::new()),
                cancel: CancellationToken::new(),
                finished: CancellationToken::new(),
                steers,
                task_actions,
            },
            incoming,
            tasks,
        )
    }
}
impl AgentRuntime {
    pub fn start(
        environment: crate::DataEnvironment,
        operations: crate::OperationSequence,
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
        let shared_model: SharedModel = Arc::new(Mutex::new(None));
        let owner_model = shared_model.clone();
        let (session_commands, mut session_incoming) = mpsc::channel(SESSION_COMMAND_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("agentelier-auth".into())
            .spawn(move || {
                debug!("认证运行线程已启动");
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async move {
                        let projection = Mutex::new(task_projection::Projection::default());
                        let deliver = publish;
                        let publish = |event: AgentEvent| {
                            let updates = match &event.kind {
                                AgentEventKind::Conversation(event) => projection.lock()
                                    .map_err(|_| DeliveryFailure::Closed)?.observe(event),
                                _ => Vec::new(),
                            };
                            deliver(event)?;
                            for event in updates {
                                deliver(AgentEvent { operation_id: 0, kind: AgentEventKind::Conversation(event) })?;
                            }
                            Ok(())
                        };
                        let data_root = environment.root().to_path_buf();
                        let store = agentelier_session_store::SessionStore::start(data_root.clone()).map_err(RuntimeFailure::Store)?;
                        let memory_root = data_root.clone();
                        let memory = tokio::task::spawn_blocking(move || Memory::open(&memory_root))
                            .await.map_err(io::Error::other).and_then(|result| result);
                        match &memory {
                            Ok(_) => debug!(path = %data_root.display(), "记忆存储已打开"),
                            Err(failure) => error!(path = %data_root.display(), error = %failure, "记忆存储不可用"),
                        }
                        let memory_processing = Arc::new(std::sync::atomic::AtomicBool::new(false));
                        let mut memory_worker = memory.as_ref().ok().and_then(|memory| memory::Background::start(
                            memory.client(), store.client(), owner_credentials.clone(), &runtime_shutdown, memory_processing.clone(),
                        ));
                        let mut mcp_settings = agentelier_mcp::ConfigStore::open(&data_root);
                        let mut material: Option<PendingMaterial> = None;
                        let mut image_attachments = tokio::task::JoinSet::new();
                        let mut audio_jobs = tokio::task::JoinSet::new();
                        let mut audio_cancel: Option<(u64, SessionId, CancellationToken)> = None;
                        let mut mcp_connections = agentelier_mcp::Connections::default();
                        if let Ok(servers) = mcp_settings.servers() {
                            mcp_connections.configure(servers);
                        }
                        let mut creating: Option<creation::Creation> = None;
                        let mut loading: Option<loading::Loading> = None;
                        let mut catalog = Some(store.client().scan_startup().map_err(RuntimeFailure::Store)?);
                        let mut startup_catalog = true;
                        let mut catalog_dirty = false;
                        let mut project_saves = std::collections::VecDeque::<(
                            u64, tokio::sync::oneshot::Receiver<io::Result<()>>,
                        )>::new();
                        let mut records = recording::Records::new();
                        let mut pending_queue = false;
                        let mut deleting: Option<deleting::Deleting> = None;
                        let mut renaming: Option<renaming::Renaming> = None;
                        let mut suspending = 0usize;
                        let mut finalizing = false;
                        let mut pending_session_model: Option<session_model::Change> = None;
                        let mut account = Account::new(owner_credentials.clone(), environment);
                        let quota_interval = std::time::Duration::from_secs(120);
                        let mut quota_tick = tokio::time::interval(quota_interval);
                        quota_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        let mut quota_due = false;
                        let mut quota_task: Option<tokio::task::JoinHandle<_>> = None;
                        let mut login = None;
                        let mut refresh: Option<tokio::task::JoinHandle<RefreshCompletion>> = None;
                        let mut logout: Option<(u64, tokio::task::JoinHandle<LogoutCompletion>)> = None;
                        let mut pending_source: Option<SourceSettlement> = None;
                        let mut requesting_catalog = None;
                        let mut model_catalog: Option<(agentelier_llm_model::CodexIdentity, Vec<agentelier_llm_model::ModelInfo>)> = None;
                        let mut delivery_failure = None;
                        let mut model: Option<ModelTask> = None;
                        let mut stopping_root = None;
                        let mut session_tools: Option<(SessionId, tool_jobs::SessionTools)> = None;
                        let mut background = background_sessions::BackgroundSessions::default();
                        let mut task_receipts = task_control::Receipts::new();
                        let (spawn_sender, mut spawn_requests) = mpsc::channel(COMMAND_CAPACITY);
                        let (mailbox_sender, mut mailbox_requests) = mpsc::channel(SESSION_COMMAND_CAPACITY);
                        let mut children = spawn_agent::Children::with_configured_limits()?;
                        let mut extensions = agentelier_memory::registrations(memory.as_ref().ok().map(Memory::client));
                        extensions.push(spawn_agent::registration(spawn_sender, owner_credentials.clone(), data_root.join("prompts.json")));
                        extensions.push(mailbox::registration(mailbox_sender.clone(), agentelier_conversation::DeliveryMode::QueueOnly));
                        extensions.push(mailbox::registration(mailbox_sender.clone(), agentelier_conversation::DeliveryMode::TriggerTurn));
                        extensions.push(wait_agent::registration(mailbox_sender.clone()));
                        extensions.push(interrupt_agent::registration(mailbox_sender.clone()));
                        extensions.push(list_agents::registration(mailbox_sender.clone()));
                        extensions.push(skill_resource::registration());
                        let user_skills = agentelier_skills::UserSkills::new(data_root.join("skills"));
                        extensions.extend(skill_creation::registrations(user_skills));
                        let mut conversation = Conversation::with_extensions(extensions);
                        let mut progress_events: Option<mpsc::Receiver<RunnerEvent>> = None;
                        let execution = async {
                            if let AccountEffect::Event(event) = account.restore().await? {
                                owner_credentials.finish_source_change()?;
                                emit(&publish, event.operation_id, AgentEventKind::Account(event.kind))?;
                            }
                            loop {
                                if model.is_none() && creating.is_none() && loading.is_none() && deleting.is_none()
                                    && suspending == 0 && !pending_queue && !finalizing && pending_source.is_none()
                                    && !runtime_shutdown.is_cancelled()
                                    && let Some(change) = pending_session_model.take()
                                {
                                    let available = model_source(&account)
                                        .and_then(|source| {
                                            conversation.validate_model_source(source)?;
                                            model_catalog.as_ref()
                                                .filter(|(owner, _)| owner == source)
                                                .map(|(_, models)| models.as_slice())
                                                .ok_or_else(|| ConversationFailure::new("请先刷新当前账号的模型目录"))
                                        });
                                    let result = match available {
                                        Ok(models) => change.save(&mut conversation, models, &store).await,
                                        Err(failure) => Err(failure),
                                    };
                                    if let Err(failure) = &result {
                                        error!(operation_id = change.id, session_id = %change.session, %failure, "会话模型保存失败");
                                    }
                                    emit(&publish, change.id, AgentEventKind::Conversation(ConversationEvent::SessionModelSaved {
                                        session: change.session,
                                        result: result.map_err(|failure| failure.to_string()),
                                    }))?;
                                    if model_source(&account).and_then(|source| conversation.validate_model_source(source)).is_ok() {
                                        model_task::start_queued(&mut conversation,&mut model,&mut progress_events,&owner_model,&owner_credentials,&publish,&data_root.join("skills"),memory.as_ref().ok().map(Memory::client),&mut session_tools,&store)?;
                                    }
                                }
                                if catalog_dirty && catalog.is_none() {
                                    catalog_dirty = false;
                                    match store.client().scan() {
                                        Ok(reply) => catalog = Some(reply),
                                        Err(failure) => emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::ArchiveFailed(failure.to_string())))?,
                                    }
                                }
                                if quota_due && quota_task.is_none() && !owner_credentials.source_settling()? {
                                    quota_due = false;
                                    if let Some(attempt) = account.quota_attempt() {
                                        quota_task = Some(tokio::spawn(attempt.run()));
                                    }
                                }
                                let idle_work = session_tools.as_ref().map(|(_, tools)| tools.work.clone());
                                let root_cancel = model.as_ref()
                                    .filter(|task| task.chat && stopping_root != Some(task.id))
                                    .map(|task| task.control.cancel.clone());
                                let command = tokio::select! { biased;
                                    _ = runtime_shutdown.cancelled() => break,
                                    result = async {
                                        if let Some((_, reply)) = project_saves.front_mut() {
                                            reply.await.map_err(|_| io::Error::other("项目存储未返回保存结果")).and_then(|result| result)
                                        } else {
                                            std::future::pending().await
                                        }
                                    }, if !project_saves.is_empty() => {
                                        let (id, _) = project_saves.pop_front().ok_or(RuntimeFailure::ModelState)?;
                                        Command::ProjectSaved(id, result)
                                    },
                                    _ = quota_tick.tick() => Command::RefreshQuota,
                                    result = async {
                                        match &mut quota_task {
                                            Some(task) => task.await,
                                            None => std::future::pending().await,
                                        }
                                    }, if quota_task.is_some() => {
                                        quota_task = None;
                                        Command::QuotaFinished(result)
                                    },
                                    _ = async move {
                                        match root_cancel {
                                            Some(cancel) => cancel.cancelled().await,
                                            None => std::future::pending().await,
                                        }
                                    } => Command::RootStopping,
                                    update = account_login::next(&mut login), if login.is_some() => update,
                                    completed = async {
                                        match &mut refresh {
                                            Some(task) => task.await,
                                            None => std::future::pending().await,
                                        }
                                    }, if refresh.is_some() => {
                                        refresh = None;
                                        Command::Account(AccountCommand::CompleteRefresh(
                                            completed.map_err(|_| "凭据续期后台任务异常结束".into()),
                                        ))
                                    },
                                    completed = async {
                                        match &mut logout {
                                            Some((id, task)) => (*id, task.await),
                                            None => std::future::pending().await,
                                        }
                                    }, if logout.is_some() => {
                                        logout = None;
                                        Command::Account(AccountCommand::CompleteLogout(
                                            completed.0,
                                            completed.1.map_err(|_| "退出登录后台任务异常结束".into()),
                                        ))
                                    },
                                    reply = async {
                                        match &mut material {
                                            Some(job) => (&mut job.task).await.unwrap_or_else(|_| Err("MCP 材料请求任务异常结束".into())),
                                            None => std::future::pending().await,
                                        }
                                    } => Command::McpMaterialFinished(reply),
                                    result = image_attachments.join_next(), if !image_attachments.is_empty() => {
                                        let (id, result) = result.ok_or(RuntimeFailure::ModelState)?
                                            .map_err(|_| RuntimeFailure::ModelState)?;
                                        Command::ImageAttachmentFinished(id, result)
                                    },
                                    result = audio_jobs.join_next(), if !audio_jobs.is_empty() => {
                                        let (id, session, result) = result.ok_or(RuntimeFailure::ModelState)?
                                            .map_err(|_| RuntimeFailure::ModelState)?;
                                        Command::AudioFinished(id, session, result)
                                    },
                                    status = mcp_connections.changed() => Command::McpStatus(status),
                                    result = async { match &mut catalog {
                                        Some(reply) => reply.await.unwrap_or_else(|_| Err(io::Error::other("会话目录读取未返回结果"))),
                                        None => std::future::pending().await,
                                    }} => Command::Scanned(result),
                                    result = async { match &mut loading {
                                        Some(pending) => pending.wait().await,
                                        None => std::future::pending().await,
                                    }} => Command::Loaded(Box::new(result)),
                                    result = async { match &mut renaming { Some(pending) => pending.wait().await, None => std::future::pending().await } } => Command::Renamed(result),
                                    result = async { match &mut deleting { Some(pending) => pending.wait().await, None => std::future::pending().await } } => Command::Deleted(result),
                                    result = records.join_next(), if !records.is_empty() => match result {
                                        Some(result) => Command::Recorded(result),
                                        None => return Err(RuntimeFailure::ModelState),
                                    },
                                    receipt = task_receipts.join_next(), if !task_receipts.is_empty() => {
                                        let (id, receipt) = receipt.ok_or(RuntimeFailure::ModelState)?
                                            .map_err(|_| RuntimeFailure::ModelState)?;
                                        Command::TaskActionFinished(id, receipt)
                                    },
                                    changed = async {
                                        match session_tools.as_mut() {
                                            Some((_, tools)) => tools.process_views.changed().await,
                                            None => std::future::pending().await,
                                        }
                                    } => {
                                        changed.map_err(|_| RuntimeFailure::ModelState)?;
                                        Command::ProcessesChanged
                                    },
                                    event = async move {
                                        match idle_work {
                                            Some(work) => work.lock().await.next().await,
                                            None => std::future::pending().await,
                                        }
                                    }, if model.is_none() && !finalizing && !pending_queue && creating.is_none() && loading.is_none() && deleting.is_none() => {
                                        Command::IdleTool(event.ok_or(RuntimeFailure::ModelState)?)
                                    },
                                    (session, signal) = background.next(), if !background.is_empty() => {
                                        Command::BackgroundSession(session, signal)
                                    },
                                    result = async { match &mut creating {
                                        Some(pending) => pending.wait().await,
                                        None => std::future::pending().await,
                                    }} => Command::Created(result),
                                    result = async { match &mut model {
                                        Some(task) => (&mut task.handle).await,
                                        None => std::future::pending().await,
                                    }} => Command::ModelFinished(result),
                                    Some(invocation) = spawn_requests.recv() => Command::SpawnAgent(Box::new(invocation)),
                                    Some(command) = mailbox_requests.recv() => Command::Mailbox(command),
                                    Some(result) = children.jobs.join_next(), if !children.jobs.is_empty() => Command::ChildFinished(result),
                                    command = session_incoming.recv(), if !pending_queue => match command {
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
                                    Command::SpawnAgent(invocation) => {
                                        children.spawn(*invocation, &mut conversation, model.as_ref(), spawn_agent::SpawnEnvironment {
                                            store: &store.client(),
                                            credentials: &owner_credentials,
                                            operations: &operations,
                                            skills_root: &data_root.join("skills"),
                                            mailbox_commands: &mailbox_sender,
                                        }).await;
                                    }
                                    Command::Mailbox(command) => {
                                        children.mailbox_command(command, &mut conversation, model.as_ref(), spawn_agent::SpawnEnvironment {
                                            store: &store.client(), credentials: &owner_credentials, operations: &operations,
                                            skills_root: &data_root.join("skills"), mailbox_commands: &mailbox_sender,
                                        }).await;
                                    }
                                    Command::ChildFinished(result) => {
                                        let ready = match children.finish(result, &store.client(), &mut conversation).await {
                                            Ok(ready) => ready,
                                            Err(failure) => {
                                                error!(%failure, "子 Agent 回收或保存未确认");
                                                if let Some(view) = conversation.fail_storage(failure.to_string()) {
                                                    if let Some(task) = &model {
                                                        task.control.cancel.cancel();
                                                    } else {
                                                        emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::IdleUpdated(view)))?;
                                                    }
                                                }
                                                None
                                            }
                                        };
                                        if let Some(agent) = ready
                                            && let Some(view) = conversation.current()
                                            && let Err(failure) = children.wake_pending(&mut conversation, view.id, agent, spawn_agent::SpawnEnvironment {
                                                store: &store.client(), credentials: &owner_credentials, operations: &operations,
                                                skills_root: &data_root.join("skills"), mailbox_commands: &mailbox_sender,
                                            }).await {
                                            tracing::error!(session_id = %view.id, agent_id = %agent, error = %failure, "唤醒待处理 Follow-up 失败");
                                        }
                                    }
                                    Command::RootStopping => {
                                        let task = model.as_ref().ok_or(RuntimeFailure::ModelState)?;
                                        stopping_root = Some(task.id);
                                        children.cancel_active();
                                        debug!(operation_id = task.id, "Root 停止已阻断协作准入，正在回收子 Agent");
                                    }
                                    Command::AudioDraft(id, session, path) => {
                                        if !audio_jobs.is_empty() || !conversation.is_current(session) {
                                            let result = Err("会话已变化或上一次音频操作尚未结束".into());
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::AudioDraft { session, result }))?;
                                        } else {
                                            let client = store.client();
                                            audio_jobs.spawn(async move { (id, session, audio_task::Reply::Draft(client.change_audio_draft(session, path).await)) });
                                        }
                                    }
                                    Command::AudioFinished(id, session, audio_task::Reply::Draft(result)) => {
                                        match &result {
                                            Ok(draft) => { conversation.apply_audio_draft(session, draft.clone()); }
                                            Err(failure) => error!(operation_id = id, session_id = %session, error = %failure, "草稿音频提交失败"),
                                        }
                                        let result = result.map_err(|failure| failure.to_string());
                                        emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::AudioDraft { session, result }))?;
                                    }
                                    Command::TranscribeAudio(id, session, audio) => {
                                        let request = if audio_jobs.is_empty() {
                                            conversation.prepare_transcription(session, audio)
                                        } else {
                                            Err(ConversationFailure::new("上一项音频操作尚未结束"))
                                        };
                                        match request {
                                            Ok((attachment, source, owner)) => {
                                                let cancel = runtime_shutdown.child_token();
                                                audio_cancel = Some((id, session, cancel.clone()));
                                                let job = audio_task::TranscriptionJob { session, attachment, source, owner, cancel };
                                                let client = store.client();
                                                let credentials = owner_credentials.clone();
                                                audio_jobs.spawn(async move { (id, session, audio_task::Reply::Transcription(job.run(client, credentials).await)) });
                                            }
                                            Err(failure) => {
                                                let result = Err(failure.to_string());
                                                emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::AudioTranscription { session, result }))?;
                                            }
                                        }
                                    }
                                    Command::CancelTranscription(id, session) => {
                                        if let Some((operation, expected, cancel)) = &audio_cancel && *operation == id && *expected == session {
                                            cancel.cancel();
                                        }
                                    }
                                    Command::AudioFinished(id, session, audio_task::Reply::Transcription(result)) => {
                                        audio_cancel = None;
                                        match &result {
                                            Ok(transcription) => {
                                                if let agentelier_conversation::TranscriptionOutcome::Failed(reason) = &transcription.outcome {
                                                    error!(operation_id = id, session_id = %session, error = reason, "音频转写失败");
                                                }
                                                conversation.apply_transcription(session, transcription.clone());
                                            }
                                            Err(failure) => error!(operation_id = id, session_id = %session, error = %failure, "转写记录提交失败"),
                                        }
                                        let result = result.map_err(|failure| failure.to_string());
                                        emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::AudioTranscription { session, result }))?;
                                    }
                                    Command::ImageAttachment(id, session, call, action) => {
                                        if let Some(image) = conversation.tool_image(session, &call) {
                                            let client = store.client();
                                            let root = data_root.clone();
                                            image_attachments.spawn(async move {
                                                (id, image_attachment::execute(client, root, session, image, action).await)
                                            });
                                        } else {
                                            emit(&publish, id, AgentEventKind::ImageAttachment(Err("当前会话没有这张已确认的图片".into())))?;
                                        }
                                    }
                                    Command::ImageAttachmentFinished(id, result) => {
                                        if let Err(failure) = &result {
                                            error!(operation_id = id, error = %failure, "处理生成图片附件失败");
                                        }
                                        emit(&publish, id, AgentEventKind::ImageAttachment(result.map_err(|failure| failure.to_string())))?;
                                    }
                                    Command::Memory(id, MemoryRequest::Reset) => {
                                        tracing::info!(operation_id = id, "已确认长期记忆重置，等待后台收敛");
                                        let result = memory::reset(memory.as_ref().ok(), &mut memory_worker, &store.client()).await;
                                        memory_processing.store(false, std::sync::atomic::Ordering::Relaxed);
                                        memory_worker = memory.as_ref().ok().and_then(|memory| memory::Background::start(
                                            memory.client(), store.client(), owner_credentials.clone(), &runtime_shutdown, memory_processing.clone(),
                                        ));
                                        match &result {
                                            Ok(_) => tracing::info!(operation_id = id, "长期记忆重置已持久提交"),
                                            Err(failure) => error!(operation_id = id, error = %failure, "长期记忆重置未确认完成"),
                                        }
                                        let reply = result.map_err(|_| "长期记忆重置未完成，详见应用日志。可重试清理。".into());
                                        emit(&publish, id, AgentEventKind::Memory(reply))?;
                                    }
                                    Command::Memory(id, request) => {
                                        let result = memory::inspect(memory.as_ref().ok(), memory_processing.load(std::sync::atomic::Ordering::Relaxed), request).await;
                                        match &result {
                                            Ok(_) => tracing::info!(operation_id = id, "已读取长期记忆展示材料"),
                                            Err(failure) => error!(operation_id = id, error = %failure, "读取长期记忆展示材料失败"),
                                        }
                                        let reply = result.map_err(|failure| match failure.kind() {
                                            io::ErrorKind::WouldBlock => failure.to_string(),
                                            io::ErrorKind::InvalidInput | io::ErrorKind::NotFound => "找不到所选长期记忆材料，请刷新后重试".into(),
                                            _ => "读取长期记忆失败，详见应用日志".into(),
                                        });
                                        emit(&publish, id, AgentEventKind::Memory(reply))?;
                                    }
                                    Command::McpMaterial(id, request) => {
                                        let cancel = CancellationToken::new();
                                        let result = if material.is_some() {
                                            Err("已有 MCP 材料操作进行中".into())
                                        } else {
                                            mcp_connections.material_request(request, cancel.clone())
                                        };
                                        match result {
                                            Ok(task) => material = Some(PendingMaterial { id, cancel, task }),
                                            Err(reason) => emit(&publish, id, AgentEventKind::McpMaterial(Err(reason)))?,
                                        }
                                    }
                                    Command::CancelMcpMaterial(id) => {
                                        if let Some(job) = &material && job.id == id {
                                            job.cancel.cancel();
                                        }
                                    }
                                    Command::McpMaterialFinished(reply) => {
                                        let job = material.take().ok_or(RuntimeFailure::ModelState)?;
                                        emit(&publish, job.id, AgentEventKind::McpMaterial(reply))?;
                                    }
                                    Command::McpConfig(id, change) => {
                                        let changes_disk = !matches!(change, agentelier_mcp::ConfigChange::Read);
                                        let result = mcp_settings.apply(change);
                                        match &result {
                                            Ok(_) if changes_disk => tracing::info!(operation_id = id, "MCP 服务器配置已保存"),
                                            Err(failure) => error!(operation_id = id, error = %failure, "MCP 配置操作失败"),
                                            Ok(_) => {}
                                        }
                                        let status = if let Ok(snapshot) = &result {
                                            if changes_disk {
                                                mcp_connections.configure(&snapshot.servers);
                                            }
                                            Some(mcp_connections.statuses())
                                        } else {
                                            None
                                        };
                                        emit(&publish, id, AgentEventKind::McpConfig(result))?;
                                        if let Some(status) = status {
                                            emit(&publish, 0, AgentEventKind::McpStatus(status))?;
                                        }
                                    }
                                    Command::McpStatus(status) => {
                                        emit(&publish, 0, AgentEventKind::McpStatus(status))?;
                                    }
                                    Command::Rename { id, session, title } => {
                                        match if renaming.is_some() || deleting.is_some() { Err(io::Error::other("已有会话管理操作正在保存")) } else { renaming::Renaming::start(id, session, title, &store) } {
                                            Ok(pending) => renaming = Some(pending),
                                            Err(failure) => emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Renamed { session, result: Err(failure.to_string()) }))?,
                                        }
                                    }
                                    Command::Renamed(result) => {
                                        let pending = renaming.take().ok_or(RuntimeFailure::ModelState)?;
                                        let id = pending.id;
                                        emit(&publish, id, AgentEventKind::Conversation(pending.complete(result)))?;
                                        if catalog.is_none() { match store.client().scan() { Ok(reply) => catalog = Some(reply), Err(failure) => emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::ArchiveFailed(failure.to_string())))? } }
                                    }

                                    Command::Deleted(result) => {
                                        let pending = deleting.take().ok_or(RuntimeFailure::ModelState)?;
                                        let id = pending.id;
                                        let event = pending.complete(result, &mut conversation);
                                        if matches!(&event, ConversationEvent::Deleted { was_open: true, .. }) && let Some((_, tools)) = session_tools.take() {
                                            tools.join("session_deleted").await?;
                                        }
                                        emit(&publish, id, AgentEventKind::Conversation(event))?;
                                    }
                                    Command::Scan => {
                                        if catalog.is_none() {
                                            match store.client().scan() {
                                                Ok(reply) => catalog = Some(reply),
                                                Err(failure) => emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::ArchiveFailed(failure.to_string())))?,
                                            }
                                        }
                                    }
                                    Command::ChangeProject(id, change) => {
                                        match store.client().change_project(change) {
                                            Ok(reply) => project_saves.push_back((id, reply)),
                                            Err(failure) => emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::ProjectSaved(Err(failure.to_string()))))?,
                                        }
                                    }
                                    Command::ChangeSessionModel(change) => {
                                        if pending_session_model.is_some() || conversation.current().is_none_or(|view| view.id != change.session) {
                                            emit(&publish, change.id, AgentEventKind::Conversation(ConversationEvent::SessionModelSaved {
                                                session: change.session,
                                                result: Err("当前已有模型变更等待保存，或目标会话已变化".into()),
                                            }))?;
                                        } else {
                                            pending_session_model = Some(change);
                                        }
                                    }
                                    Command::ProjectSaved(id, result) => {
                                        match &result {
                                            Ok(()) => {
                                                catalog_dirty = true;
                                                tracing::info!(operation_id = id, "项目操作已持久保存");
                                            }
                                            Err(failure) => error!(operation_id = id, error = %failure, "项目保存失败"),
                                        }
                                        emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::ProjectSaved(result.map_err(|failure| failure.to_string()))))?;
                                    }
                                    Command::Scanned(result) => {
                                        catalog = None;
                                        match result {
                                            Ok(catalog) => {
                                                emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::Projects(catalog.projects)))?;
                                                let entries = catalog.entries;
                                                let first = entries.first().map(|entry| entry.summary.session);
                                                emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::Archives(entries)))?;
                                                if !catalog.task_history.is_empty() {
                                                    let history = projection.lock().map_err(|_| AccountFailure::Delivery(DeliveryFailure::Closed))?
                                                        .seed_history(catalog.task_history);
                                                    if let Some(history) = history {
                                                        emit(&publish, 0, AgentEventKind::Conversation(history))?;
                                                    }
                                                }
                                                if startup_catalog && conversation.current().is_none() && model.is_none() && creating.is_none() && loading.is_none() && deleting.is_none() && suspending == 0 && !finalizing && !pending_queue && let Some(session) = first {
                                                        match loading::Loading::start(0, None, session, &store) {
                                                            Ok(pending) => loading = Some(pending),
                                                            Err(failure) => emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::ArchiveFailed(failure.to_string())))?,
                                                        }
                                                }
                                                emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::ArchiveCleanup(catalog.cleanup_errors)))?;
                                                startup_catalog = false;
                                            }
                                            Err(failure) => emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::ArchiveFailed(format!("无法读取本地会话目录：{failure}"))))?,
                                        }
                                    }
                                    Command::Loaded(result) => {
                                        let pending = loading.take().ok_or(RuntimeFailure::ModelState)?;
                                        let id = pending.id;
                                        let mut next = conversation.empty_session_owner();
                                        let event = pending.complete(*result, &operations, &mut next, &mcp_connections);
                                        let opened = matches!(&event, ConversationEvent::Opened(_));
                                        if opened {
                                            let previous = std::mem::replace(&mut conversation, next);
                                            background.detach(previous, session_tools.take()).await?;
                                        }
                                        emit(&publish, id, AgentEventKind::Conversation(event))?;
                                        if opened {
                                            model_task::publish_tasks(&conversation, &publish)?;
                                            if let Some(view) = conversation.current() {
                                                let processes = conversation.restored_processes().projection(view.id);
                                                emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::Processes(processes)))?;
                                            }
                                        }
                                        if opened && catalog.is_none() {
                                            match store.client().scan() {
                                                Ok(reply) => catalog = Some(reply),
                                                Err(failure) => emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::ArchiveFailed(failure.to_string())))?,
                                            }
                                        }
                                    }
                                    Command::Recorded(result) => {
                                        let (pending, result) = result.map_err(|_| RuntimeFailure::ModelState)?;
                                        let applied = recording::apply(pending, result, &mut conversation, &publish)?;
                                        match applied.kind { recording::AppliedKind::Suspended => suspending = suspending.checked_sub(1).ok_or(RuntimeFailure::ModelState)?, recording::AppliedKind::Queue => pending_queue = false, recording::AppliedKind::Finished => finalizing = false }
                                        if applied.failed && let Some(control) = owner_model.lock().map_err(|_| RuntimeFailure::ModelState)?.as_ref() { control.cancel.cancel(); }
                                        if applied.failed && let Some((_, tools)) = &session_tools {
                                            tools.admission.block();
                                            if let Ok(work) = tools.work.try_lock() {
                                                work.stop_all();
                                            }
                                        }
                                        if applied.failed && pending_source.is_some() {
                                            return Err(RuntimeFailure::Store(io::Error::other("凭据来源切换前的会话收尾保存未确认")));
                                        }
                                        if model.is_none() && creating.is_none() && loading.is_none() && deleting.is_none() && suspending == 0 && !pending_queue && !finalizing && pending_source.is_none() && pending_session_model.is_none() && !runtime_shutdown.is_cancelled()
                                            && model_source(&account).and_then(|source| conversation.validate_model_source(source)).is_ok() {
                                            model_task::start_queued(&mut conversation,&mut model,&mut progress_events,&owner_model,&owner_credentials,&publish,&data_root.join("skills"),memory.as_ref().ok().map(Memory::client),&mut session_tools,&store)?;
                                        }
                                    }
                                    Command::Created(result) => {
                                        let pending = creating.take().ok_or(RuntimeFailure::ModelState)?;
                                        let id = pending.id;
                                        let mut next = conversation.empty_session_owner();
                                        let event = pending.complete(result, &mut next);
                                        if matches!(&event, ConversationEvent::Created(_)) {
                                            let previous = std::mem::replace(&mut conversation, next);
                                            background.detach(previous, session_tools.take()).await?;
                                        }
                                        emit(&publish, id, AgentEventKind::Conversation(event))?;
                                    }
                                    Command::Model {
                                        id,
                                        control,
                                        request,
                                    } => {
                                        if model.is_some() || creating.is_some() || loading.is_some() || deleting.is_some() || suspending != 0 || finalizing || pending_queue {
                                            control.finished.cancel();
                                            emit(&publish,id,AgentEventKind::ModelCompleted(Err(agentelier_llm_model::ModelFailure {usage:None,response_id:None,kind:agentelier_llm_model::ModelFailureKind::Service,status:None,code:None,request_id:None,message:"当前任务正在执行，不能插入模型操作".into()})))?;
                                            continue;
                                        }
                                        if matches!(&request, ModelRequest::Catalog) {
                                            model_catalog = None;
                                            requesting_catalog = account.codex_identity().cloned();
                                        }
                                        *owner_model.lock().map_err(|_| RuntimeFailure::ModelState)?=Some(control.clone());
                                        model = Some(ModelTask::start(
                                            id,
                                            control,
                                            owner_credentials.clone(),
                                            TaskRequest::Model(request),
                                        ));
                                    }
                                    Command::ModelFinished(result) => {
                                        let task =
                                            model.take().ok_or(RuntimeFailure::ModelState)?;
                                        task.check_instance(&conversation)?;
                                        stopping_root = None;
                                        if let Some(bound) = task.control.turn.get() {
                                            children.clear_wait(agentelier_conversation::AgentId::for_root(bound.session));
                                        }
                                        debug!(operation_id = task.id, "模型操作已真实结束并回收");
                                        drain_progress(&mut conversation, task.id, &mut progress_events, &store.client(), &publish)?;
                                        let event = complete(&mut conversation, task.id, task.chat, result)?;
                                        if let Some(source) = requesting_catalog.take()
                                            && account.codex_identity() == Some(&source)
                                            && let AgentEventKind::ModelCompleted(Ok(ModelResult::Catalog(models))) = &event
                                        {
                                            model_catalog = Some((source, models.clone()));
                                        }
                                        if task.chat {
                                            if task.control.cancel.is_cancelled()
                                                && let Err(failure) = children.settle_active(&store.client(), &mut conversation, Some(&mut mailbox_requests)).await
                                            {
                                                error!(operation_id = task.id, %failure, "Root 停止后的 Agent Tree 收尾未确认");
                                                conversation.fail_storage(failure.to_string());
                                            }
                                            if conversation.current().is_some_and(|view| view.execution_blocked)
                                                && let Some((_, tools)) = &session_tools {
                                                tools.admission.block();
                                                tools.work.lock().await.stop_all();
                                            }
                                            let (state, history) = conversation.archive().ok_or(RuntimeFailure::ModelState)?;
                                            let session = state.session;
                                            recording::submit(recording::Pending::Finished { id: task.id, session, control: task.control }, agentelier_conversation::RecordEvent::Finished { state, history }, &store, &mut records);
                                            finalizing = true;
                                        } else {
                                            task.control.finished.cancel();
                                            capture_delivery(emit(&publish, task.id, event), &mut delivery_failure);
                                        }
                                        if !finalizing && !pending_queue && suspending == 0 && pending_source.is_none() && pending_session_model.is_none() && delivery_failure.is_none() && !runtime_shutdown.is_cancelled()
                                            && model_source(&account).and_then(|source| conversation.validate_model_source(source)).is_ok() {
                                            model_task::start_queued(&mut conversation,&mut model,&mut progress_events,&owner_model,&owner_credentials,&publish,&data_root.join("skills"),memory.as_ref().ok().map(Memory::client),&mut session_tools,&store)?;
                                        }
                                    }
                                    Command::Session(command) => {
                                        if creating.is_some() || loading.is_some() || deleting.is_some() || suspending != 0 || finalizing || (matches!(command,SessionCommand::Resume {..}) && model.is_some()) {
                                            let id=command.id();
                                            emit(&publish,id,AgentEventKind::Conversation(ConversationEvent::Queue(agentelier_conversation::QueueEvent {
                                                id,view:None,decision:agentelier_conversation::QueueDecision::Rejected("当前执行尚未回收，不能继续队列".into()),
                                            })))?;
                                            continue;
                                        }
                                        if matches!(command, SessionCommand::Resume { .. })
                                            && let Err(failure) = model_source(&account).and_then(|source| conversation.validate_model_source(source))
                                        {
                                            let id = command.id();
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Queue(agentelier_conversation::QueueEvent { id, view: conversation.current(), decision: agentelier_conversation::QueueDecision::Rejected(failure.to_string()) })))?;
                                            continue;
                                        }
                                        match conversation.prepare_queue(command) {
                                            Ok(pending) => {
                                                let event = pending.record();
                                                recording::submit(recording::Pending::Queue(pending), event, &store, &mut records);
                                                pending_queue = true;
                                            }
                                            Err(event) => emit(&publish,event.id,AgentEventKind::Conversation(ConversationEvent::Queue(*event)))?,
                                        }
                                    }
                                    Command::ProcessesChanged => {
                                        if let Some((session, tools)) = &session_tools {
                                            let processes = tools.process_views.borrow().projection(*session);
                                            emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::Processes(processes)))?;
                                        }
                                    }
                                    Command::IdleTool(event) => {
                                        let (_, tools) = session_tools.as_ref().ok_or(RuntimeFailure::ModelState)?;
                                        if let Err(error) = model_task::drive_idle(&mut conversation, tools, agentelier_turn_runner::IdleAction::Tool(event), &store.client(), &publish).await {
                                            tools.admission.block();
                                            tracing::error!(%error, "后台工具结果保存或归并失败");
                                            if let Some(view) = conversation.fail_storage(error.to_string()) {
                                                emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::IdleUpdated(view)))?;
                                            }
                                        }
                                    }
                                    Command::BackgroundSession(session, signal) => {
                                        background.handle(session, signal, &store.client(), &publish).await?;
                                    }
                                    Command::TaskActionFinished(id, receipt) => {
                                        model_task::publish_tasks(&conversation, &publish)?;
                                        emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::TaskAction(receipt)))?;
                                    }
                                    Command::TaskAction { id, session, action } => {
                                        if creating.is_some() || loading.is_some() || deleting.is_some() || finalizing || suspending != 0 || pending_queue {
                                            task_control::reject(&mut task_receipts, id, session, "会话变更或保存正在进行");
                                        } else {
                                            let tools = session_tools.as_ref().filter(|(current, _)| *current == session).map(|(_, tools)| tools);
                                            task_control::dispatch(&mut task_receipts, id, session, action, &mut conversation, model.as_ref(), tools, &store, &publish).await;
                                        }
                                    }
                                    Command::Stream(event) => {
                                        if let Some(RunnerEvent::Inbox(request)) = event {
                                            if model.as_ref().is_some_and(|task| task.control.cancel.is_cancelled()) {
                                                children.settle_inbox(request, &mut conversation, model.as_ref(), &store.client()).await;
                                            } else {
                                                children.inbox(request, &mut conversation, model.as_ref(), &store.client()).await;
                                            }
                                        } else if let Some(event) = event {
                                            let task = model
                                                .as_ref()
                                                .filter(|task| task.chat)
                                                .ok_or(RuntimeFailure::ModelState)?;
                                            task.check_instance(&conversation)?;
                                            if matches!(&event, RunnerEvent::Receipt(receipt) if matches!(receipt.decision, agentelier_conversation::InputDecision::Accepted))
                                                && let Some(bound) = task.control.turn.get() {
                                                children.steer_wait(agentelier_conversation::AgentId::for_root(bound.session));
                                            }
                                            progress(&mut conversation, task.id, event, &store.client(), &publish)?;
                                        } else {
                                            progress_events = None;
                                        }
                                    }
                                    Command::Conversation {
                                        id,
                                        control,
                                        request,
                                        steers,
                                        task_actions,
                                    } => {
                                        let replacing = match &request {
                                            ConversationRequest::Delete { session } => conversation.current().is_some_and(|current| current.id == *session),
                                            ConversationRequest::Create(_) | ConversationRequest::Open { .. } | ConversationRequest::Reset { .. } | ConversationRequest::SetWorkingDirectory { .. } => true,
                                            _ => false,
                                        };
                                        let can_park = matches!(&request, ConversationRequest::Open { .. } | ConversationRequest::Create(_));
                                        if let ConversationRequest::Delete { session } = &request
                                            && background.contains(*session)
                                        {
                                            control.finished.cancel();
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected("该会话仍有后台任务或进程，结束后可删除".into())))?;
                                            continue;
                                        }
                                        if replacing && children.has_active() {
                                            control.finished.cancel();
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected("当前会话仍有子 Agent 正在执行".into())))?;
                                            continue;
                                        }
                                        if replacing && let Err(failure) = conversation.check_replacement() {
                                            control.finished.cancel();
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected(failure.to_string())))?;
                                            continue;
                                        }
                                        if replacing && !can_park && session_tools.as_ref().is_some_and(|(_, tools)| tools.process_views.borrow().has_active()) {
                                            control.finished.cancel();
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected("当前会话仍有进程活跃或回收未确认".into())))?;
                                            continue;
                                        }
                                        if replacing && !can_park && session_tools.as_ref().is_some_and(|(_, tools)| tools.busy()) {
                                            control.finished.cancel();
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected("当前会话仍有后台任务或结果正在保存".into())))?;
                                            continue;
                                        }
                                        if replacing && !task_receipts.is_empty() {
                                            control.finished.cancel();
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected("仍有任务控制正在确认结果".into())))?;
                                            continue;
                                        }
                                        if replacing && matches!(&request, ConversationRequest::Reset { .. } | ConversationRequest::Delete { .. })
                                            && conversation.current().is_some_and(|view| view.execution_blocked)
                                        {
                                            control.finished.cancel();
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected("当前会话有未确认的执行或保存结果".into())))?;
                                            continue;
                                        }
                                        if model.is_some() || creating.is_some() || loading.is_some() || deleting.is_some() || suspending != 0 || finalizing || pending_queue {
                                            control.finished.cancel();
                                            let reason = if replacing { "任务正在回收或会话保存正在进行，暂不能替换会话" } else { "排队任务已经开始，当前发送未接纳，草稿保留" };
                                            emit(&publish,id,AgentEventKind::Conversation(ConversationEvent::Rejected(reason.into())))?;
                                            continue;
                                        }
                                        if replacing && !matches!(&request, ConversationRequest::SetWorkingDirectory { .. }) {
                                            children.release_idle(&mut conversation)?;
                                        }
                                        match request {
                                            ConversationRequest::Delete { session } => {
                                                match if renaming.is_some() { Err(io::Error::other("名称写入尚未完成，请稍后删除")) } else { deleting::Deleting::start(id, control.clone(), session, &store) } {
                                                    Ok(pending) => { *owner_model.lock().map_err(|_| RuntimeFailure::ModelState)? = Some(control); deleting = Some(pending); }
                                                    Err(failure) => { control.finished.cancel(); emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected(failure.to_string())))?; }
                                                }
                                            }
                                            ConversationRequest::Open { session } => {
                                                if let Some(view) = conversation.current().filter(|view| view.id == session) {
                                                    control.finished.cancel();
                                                    emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Opened(view)))?;
                                                    continue;
                                                }
                                                if let Some(resident) = background.take(session) {
                                                    let previous = std::mem::replace(&mut conversation, resident.conversation);
                                                    let previous_tools = session_tools.replace((session, resident.tools));
                                                    background.detach(previous, previous_tools).await?;
                                                    let view = conversation.current().ok_or(RuntimeFailure::ModelState)?;
                                                    control.finished.cancel();
                                                    debug!(%session, "重新选中已保留后台资源的会话");
                                                    emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Opened(view)))?;
                                                    model_task::publish_tasks(&conversation, &publish)?;
                                                    if let Some((_, tools)) = &session_tools {
                                                        let processes = tools.process_views.borrow().projection(session);
                                                        emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::Processes(processes)))?;
                                                    }
                                                    continue;
                                                }
                                                match loading::Loading::start(id, Some(control.clone()), session, &store) {
                                                    Ok(pending) => {
                                                        *owner_model.lock().map_err(|_| RuntimeFailure::ModelState)? = Some(control);
                                                        loading = Some(pending);
                                                    }
                                                    Err(failure) => {
                                                        control.finished.cancel();
                                                        emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::ArchiveFailed(failure.to_string())))?;
                                                    }
                                                }
                                            }
                                            ConversationRequest::SetWorkingDirectory { session, directory } => {
                                                let result = creation::save_working_directory(id, session, &directory, &mut conversation, &store).await;
                                                control.finished.cancel();
                                                let event = match result {
                                                    Ok(view) => ConversationEvent::WorkingDirectorySaved(view),
                                                    Err(failure) => ConversationEvent::Rejected(failure.to_string()),
                                                };
                                                emit(&publish, id, AgentEventKind::Conversation(event))?;
                                            }
                                            request @ (ConversationRequest::Create(_) | ConversationRequest::Reset { .. }) => {
                                                let source = match model_source(&account) {
                                                    Ok(source) => source.clone(),
                                                    Err(failure) => {
                                                        control.finished.cancel();
                                                        emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected(failure.to_string())))?;
                                                        continue;
                                                    }
                                                };
                                                if matches!(&request, ConversationRequest::Reset { .. })
                                                    && let Err(failure) = conversation.validate_model_source(&source)
                                                {
                                                    control.finished.cancel();
                                                    emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected(failure.to_string())))?;
                                                    continue;
                                                }
                                                let snapshot = if let Ok(memory) = &memory {
                                                    match memory.client().snapshot().await {
                                                        Ok(snapshot) => snapshot,
                                                        Err(failure) => {
                                                            error!(operation_id = id, error = %failure, "无法取得已发布的会话记忆");
                                                            None
                                                        }
                                                    }
                                                } else { None };
                                                let tools = mcp_connections.registrations();
                                                let prepared = match request {
                                                    ConversationRequest::Create(create) => async {
                                                        use agentelier_conversation::SessionLocation;
                                                        let (_, models) = model_catalog.as_ref()
                                                            .filter(|(owner, _)| owner == &source)
                                                            .ok_or_else(|| ConversationFailure::new("请先刷新当前账号的模型目录"))?;
                                                        let (project, directory) = match &create.location {
                                                            SessionLocation::Standalone(directory) => (None, directory.clone()),
                                                            SessionLocation::InProject { project, directory_override } => {
                                                                let directory = store.project_directory(*project).await
                                                                    .map_err(|failure| ConversationFailure::new(format!("无法读取目标项目：{failure}")))?;
                                                                (Some(*project), directory_override.clone().unwrap_or(directory))
                                                            }
                                                        };
                                                        let settings = create.resolve(source, models, directory)?;
                                                        conversation.prepare(settings, &tools, snapshot).map(|prepared| (prepared, project))
                                                    }.await,
                                                    ConversationRequest::Reset { session } => conversation.prepare_reset(session, &tools, snapshot).map(|prepared| (prepared, None)),
                                                    _ => return Err(RuntimeFailure::ModelState),
                                                };
                                                match prepared.and_then(|(prepared, project)| creation::Creation::start(id, control.clone(), prepared, project, &store)) {
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
                                            request @ (ConversationRequest::Send { .. } | ConversationRequest::Retry { .. } | ConversationRequest::ContinueWithoutCompaction { .. } | ConversationRequest::ProcessResults { .. }) => {
                                                if let Err(failure) = model_source(&account).and_then(|source| conversation.validate_model_source(source)) {
                                                    control.finished.cancel();
                                                    emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected(failure.to_string())))?;
                                                    continue;
                                                }
                                                match conversation.start(id, request) {
                                                    Ok(view) => {
                                                        let tools = tool_jobs::SessionTools::for_session(&mut session_tools, &view, &conversation, store.client());
                                                        control.bind_root(&mut conversation, &view)?;
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
                                                            TaskRequest::Chat(sender, steers, task_actions, data_root.join("skills"), memory.as_ref().ok().map(Memory::client), tools),
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
                                    Command::BeginLogin(id) => {
                                        if login.is_some() {
                                            emit(&publish, id, AgentEventKind::Account(
                                                AccountEventKind::SourceChanged(Err("已有浏览器登录正在进行".into())),
                                            ))?;
                                        } else {
                                            login = Some(account_login::LoginTask::start(id, &runtime_shutdown));
                                        }
                                    }
                                    Command::CancelLogin(id) => {
                                        if let Some(task) = &login {
                                            task.cancel(id);
                                        }
                                    }
                                    Command::LoginWaiting(id, prompt) => {
                                        emit(&publish, id, AgentEventKind::Account(AccountEventKind::LoginWaiting(prompt)))?;
                                    }
                                    Command::RefreshQuota => quota_due = true,
                                    Command::QuotaFinished(result) => match result {
                                        Ok(result) => {
                                            if let Some(quota) = account.accept_quota(result) {
                                                emit(&publish, 0, AgentEventKind::Account(AccountEventKind::Quota(Box::new(quota))))?;
                                            }
                                        }
                                        Err(failure) if failure.is_cancelled() => {}
                                        Err(failure) => error!(%failure, "Codex 账号额度后台请求异常结束"),
                                    },
                                    Command::Account(command) => {
                                        let renewing = matches!(&command, AccountCommand::CompleteRefresh(_));
                                        let previous_version = renewing.then(|| account.quota_version()).flatten();
                                        let (mut source_event, logout_request) = match account.handle(command).await? {
                                            AccountEffect::Event(event) => (Some(event), None),
                                            AccountEffect::Refresh(attempt) => {
                                                refresh = Some(tokio::spawn(attempt.run()));
                                                (None, None)
                                            }
                                            AccountEffect::LogoutSettling(id) => (None, Some(id)),
                                            AccountEffect::None => (None, None),
                                        };
                                        if renewing && previous_version != account.quota_version()
                                            || source_event.as_ref().is_some_and(|event| matches!(&event.kind, AccountEventKind::SourceChanged(Ok(_))))
                                        {
                                            quota_due = true;
                                        }
                                        let settling = owner_credentials.source_settling()?;
                                        let starting_settlement = settling && pending_source.is_none() && logout.is_none();
                                        if let Some(id) = logout_request {
                                            pending_source = Some(SourceSettlement::Logout(id));
                                            emit(&publish, id, AgentEventKind::Account(AccountEventKind::SourceSettling))?;
                                        } else if settling
                                            && source_event.as_ref().is_some_and(|event| matches!(event.kind, AccountEventKind::SourceChanged(Ok(_))))
                                            && let Some(event) = source_event.take()
                                        {
                                            if starting_settlement {
                                                emit(&publish, 0, AgentEventKind::Account(AccountEventKind::SourceSettling))?;
                                                debug!("凭据来源已变化，开始回收旧任务并保存待办");
                                            }
                                            pending_source = Some(SourceSettlement::Publish(event));
                                        }
                                        if starting_settlement {
                                            if let Some(task) = &quota_task {
                                                task.abort();
                                            }
                                            requesting_catalog = None;
                                            model_catalog = None;
                                            if let Some(task) = &model {
                                                task.control.cancel.cancel();
                                            }
                                            children.settle_active(&store.client(), &mut conversation, Some(&mut mailbox_requests)).await?;
                                            if model.is_some() { stop_model(&mut model, &mut progress_events, &mut conversation, &store, &publish).await?; }
                                            if let Some((session, tools)) = session_tools.take() {
                                                let processes = tools.process_views.clone();
                                                model_task::finish_tools(tools, "account_changed", &mut conversation, &store.client(), &publish).await?;
                                                let update = conversation.retain_closed_processes(session, processes.borrow().clone())?;
                                                emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::Processes(update)))?;
                                            }
                                            background.finish("account_changed", &store.client(), &publish).await?;
                                            if let Some(event) = conversation.suspend_queue()? {
                                                let session = event.session();
                                                recording::submit(recording::Pending::Suspended { session }, event, &store, &mut records);
                                                suspending = suspending.checked_add(1).ok_or(RuntimeFailure::ModelState)?;
                                            }
                                        }
                                        if let Some(event) = source_event {
                                            capture_delivery(
                                                emit(&publish, event.operation_id, AgentEventKind::Account(event.kind)),
                                                &mut delivery_failure,
                                            );
                                        }
                                    }
                                }
                                if suspending == 0 && !pending_queue && !finalizing
                                    && (refresh.is_none() || matches!(pending_source, Some(SourceSettlement::Publish(_))))
                                    && let Some(settlement) = pending_source.take()
                                {
                                    match settlement {
                                        SourceSettlement::Publish(event) => {
                                            owner_credentials.finish_source_change()?;
                                            debug!("旧账号任务收尾完成，已发布当前凭据来源");
                                            emit(&publish, event.operation_id, AgentEventKind::Account(event.kind))?;
                                        }
                                        SourceSettlement::Logout(id) => {
                                            let attempt = account.logout_attempt(id);
                                            logout = Some((id, tokio::spawn(attempt.run())));
                                        }
                                    }
                                }
                                if delivery_failure.is_some() {
                                    break;
                                }
                            }
                            Ok::<(), RuntimeFailure>(())
                        }
                        .await;
                        if let Some(task) = quota_task {
                            task.abort();
                            drop(task.await);
                        }
                        if let Some(login) = login {
                            login.finish().await;
                        }
                        if let Some(refresh) = refresh {
                            let completed = refresh.await.map_err(|_| "凭据续期后台任务异常结束".into());
                            if let Err(failure) = account.handle(AccountCommand::CompleteRefresh(completed)).await {
                                capture_delivery(Err(failure.into()), &mut delivery_failure);
                            }
                        }
                        drop(spawn_requests);
                        if let Some(task) = &model {
                            task.control.cancel.cancel();
                        }
                        capture_delivery(
                            children.stop(&store.client(), &mut conversation, &mut mailbox_requests).await,
                            &mut delivery_failure,
                        );
                        drop(mailbox_requests);
                        drop(children);
                        capture_delivery(
                            stop_model(
                                &mut model,
                                &mut progress_events,
                                &mut conversation,
                                &store,
                                &publish,
                            )
                            .await,
                            &mut delivery_failure,
                        );
                        if let Some((_, tools)) = session_tools.take() {
                            capture_delivery(model_task::finish_tools(tools, "application_shutdown", &mut conversation, &store.client(), &publish).await, &mut delivery_failure);
                        }
                        capture_delivery(background.finish("application_shutdown", &store.client(), &publish).await, &mut delivery_failure);
                        if let Some(job) = material {
                            job.cancel.cancel();
                            if job.task.await.is_err() {
                                error!(operation_id = job.id, "关闭时 MCP 材料请求任务异常结束");
                            }
                        }
                        while let Some(result) = image_attachments.join_next().await {
                            match result {
                                Ok((id, Err(failure))) => error!(operation_id = id, error = %failure, "关闭期间处理生成图片附件失败"),
                                Err(failure) => error!(error = %failure, "关闭期间生成图片附件任务异常结束"),
                                Ok((_, Ok(_))) => {}
                            }
                        }
                        if let Some((_, _, cancel)) = audio_cancel { cancel.cancel(); }
                        while let Some(result) = audio_jobs.join_next().await {
                            match result {
                                Ok((id, session, audio_task::Reply::Draft(Err(failure)) | audio_task::Reply::Transcription(Err(failure)))) => error!(operation_id = id, session_id = %session, error = %failure, "关闭期间音频记录提交失败"),
                                Err(failure) => error!(error = %failure, "关闭期间音频操作异常结束"),
                                Ok(_) => {}
                            }
                        }
                        let mcp_finished = mcp_connections.shutdown().await;
                        let memory_worker_finished = if let Some(worker) = memory_worker {
                            worker.stop().await
                        } else { Ok(()) };
                        while let Some(result) = records.join_next().await {
                            match result {
                                Ok((pending, result)) => capture_delivery(recording::apply(pending, result, &mut conversation, &publish).map(|_| ()), &mut delivery_failure),
                                Err(_) => { delivery_failure = delivery_failure.or(Some(RuntimeFailure::ModelState)); }
                            }
                        }
                        if let Some(mut pending) = renaming { let result = pending.wait().await; let id = pending.id; capture_delivery(emit(&publish, id, AgentEventKind::Conversation(pending.complete(result))), &mut delivery_failure); }
                        if let Some(mut pending) = deleting { let result = pending.wait().await; let id = pending.id; capture_delivery(emit(&publish, id, AgentEventKind::Conversation(pending.complete(result, &mut conversation))), &mut delivery_failure); }
                        while let Some(receipt) = task_receipts.join_next().await {
                            if let Err(failure) = receipt {
                                tracing::error!(%failure, "任务控制收尾失败");
                                delivery_failure = delivery_failure.or(Some(RuntimeFailure::ModelState));
                            }
                        }
                        drop(conversation);
                        let stored = store.finish().await.map_err(RuntimeFailure::Store);
                        if stored.is_ok() {
                            debug!("会话存储已排空并关闭");
                        }
                        let logout_completion = match logout {
                            Some((id, task)) => Some((id, task.await.map_err(|_| "退出登录后台任务异常结束".into()))),
                            None if stored.is_ok() && execution.is_ok() && delivery_failure.is_none() => {
                                match pending_source {
                                    Some(SourceSettlement::Logout(id)) => Some((id, Ok(account.logout_attempt(id).run().await))),
                                    _ => None,
                                }
                            }
                            None => None,
                        };
                        if let Some((id, completed)) = logout_completion
                            && let Err(failure) = account.handle(AccountCommand::CompleteLogout(id, completed)).await
                        {
                            capture_delivery(Err(failure.into()), &mut delivery_failure);
                        }
                        if let Some(pending) = loading && let Some(control) = pending.control { control.finished.cancel(); }
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
                        let memory_finished = if let Ok(memory) = memory {
                            let result = tokio::task::spawn_blocking(move || memory.close())
                                .await.map_err(io::Error::other).and_then(|result| result);
                            match &result {
                                Ok(()) => debug!("记忆存储已关闭"),
                                Err(failure) => error!(error = %failure, "记忆存储收尾失败"),
                            }
                            result
                        } else {
                            Ok(())
                        };
                        if let Err(failure) = stored {
                            error!(error = %failure, "会话存储收尾失败");
                            execution?;
                            return Err(failure);
                        }
                        execution?;
                        pending_saved?;
                        mcp_finished.map_err(RuntimeFailure::Mcp)?;
                        memory_worker_finished.map_err(RuntimeFailure::Memory)?;
                        memory_finished.map_err(RuntimeFailure::Memory)?;
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
            credentials,
            shutdown,
            model: shared_model,
            session_commands,
        })
    }

    pub fn request_memory(&self, id: u64, request: MemoryRequest) -> Result<(), RuntimeFailure> {
        self.send(Command::Memory(id, request))
    }

    pub fn request_image_attachment(
        &self,
        id: u64,
        session: SessionId,
        call: String,
        action: ImageAttachmentAction,
    ) -> Result<(), RuntimeFailure> {
        let preview = matches!(&action, ImageAttachmentAction::Preview);
        let command = Command::ImageAttachment(id, session, call, action);
        if preview {
            self.submit(command)
        } else {
            self.send(command)
        }
    }

    pub fn change_audio_draft(
        &self,
        id: u64,
        session: SessionId,
        path: Option<PathBuf>,
    ) -> Result<(), RuntimeFailure> {
        self.send(Command::AudioDraft(id, session, path))
    }

    pub fn transcribe_audio(
        &self,
        id: u64,
        session: SessionId,
        audio: AudioId,
    ) -> Result<(), RuntimeFailure> {
        self.send(Command::TranscribeAudio(id, session, audio))
    }

    pub fn cancel_transcription(&self, id: u64, session: SessionId) -> Result<(), RuntimeFailure> {
        self.send(Command::CancelTranscription(id, session))
    }

    pub fn mcp_material(
        &self,
        id: u64,
        request: agentelier_mcp::MaterialRequest,
    ) -> Result<(), RuntimeFailure> {
        self.send(Command::McpMaterial(id, request))
    }

    pub fn cancel_mcp_material(&self, id: u64) -> Result<(), RuntimeFailure> {
        self.send(Command::CancelMcpMaterial(id))
    }

    pub fn mcp_config(&self, id: u64, change: agentelier_mcp::ConfigChange) -> io::Result<()> {
        self.commands
            .as_ref()
            .ok_or_else(|| io::Error::other("执行连接已经关闭"))?
            .try_send(Command::McpConfig(id, change))
            .map_err(|failure| match failure {
                mpsc::error::TrySendError::Full(_) => io::Error::other("执行连接命令队列已满"),
                mpsc::error::TrySendError::Closed(_) => io::Error::other("执行连接已经关闭"),
            })
    }

    pub fn begin_login(&self, id: u64) -> Result<(), RuntimeFailure> {
        self.send(Command::BeginLogin(id))
    }
    pub fn cancel_login(&self, id: u64) -> Result<(), RuntimeFailure> {
        self.submit(Command::CancelLogin(id))
    }
    pub fn logout(&self, id: u64) -> Result<(), RuntimeFailure> {
        self.send(Command::Account(AccountCommand::BeginLogout(id)))
    }
    pub fn credentials(&self) -> CredentialClient {
        self.credentials.clone()
    }
    pub fn request_model(&mut self, id: u64, request: ModelRequest) -> Result<(), RuntimeFailure> {
        let (control, steers, task_actions) = ModelControl::new(id);
        drop((steers, task_actions));
        self.send(Command::Model {
            id,
            request,
            control: control.clone(),
        })?;
        Ok(())
    }
    pub fn change_session_model(
        &self,
        id: u64,
        session: SessionId,
        slug: String,
    ) -> Result<(), RuntimeFailure> {
        self.submit(Command::ChangeSessionModel(session_model::Change {
            id,
            session,
            slug,
        }))
    }
    pub fn request_conversation(
        &mut self,
        id: u64,
        request: ConversationRequest,
    ) -> Result<(), RuntimeFailure> {
        let (control, steers, task_actions) = ModelControl::new(id);
        let replacing = matches!(
            request,
            ConversationRequest::Create(_)
                | ConversationRequest::Open { .. }
                | ConversationRequest::Reset { .. }
                | ConversationRequest::SetWorkingDirectory { .. }
                | ConversationRequest::Delete { .. }
        );
        let command = Command::Conversation {
            steers,
            task_actions,
            id,
            control,
            request,
        };
        if replacing {
            self.submit(command)
        } else {
            self.send(command)
        }
    }
    pub fn request_task_action(
        &self,
        id: u64,
        session: SessionId,
        action: agentelier_conversation::TaskAction,
    ) -> Result<(), RuntimeFailure> {
        self.submit(Command::TaskAction {
            id,
            session,
            action,
        })
    }

    pub fn rename_session(
        &self,
        id: u64,
        session: SessionId,
        title: String,
    ) -> Result<(), RuntimeFailure> {
        if self.shutdown.is_cancelled() {
            return Err(AccountFailure::Closed.into());
        }
        self.commands
            .as_ref()
            .ok_or(AccountFailure::Closed)?
            .try_send(Command::Rename { id, session, title })
            .map_err(|failure| match failure {
                mpsc::error::TrySendError::Full(_) => AccountFailure::Busy,
                mpsc::error::TrySendError::Closed(_) => AccountFailure::Closed,
            })?;
        Ok(())
    }
    pub fn refresh_archives(&self) -> Result<(), RuntimeFailure> {
        if self.shutdown.is_cancelled() {
            return Err(AccountFailure::Closed.into());
        }
        self.commands
            .as_ref()
            .ok_or(AccountFailure::Closed)?
            .try_send(Command::Scan)
            .map_err(|failure| match failure {
                mpsc::error::TrySendError::Full(_) => AccountFailure::Busy,
                mpsc::error::TrySendError::Closed(_) => AccountFailure::Closed,
            })?;
        Ok(())
    }
    pub fn change_project(
        &self,
        id: u64,
        change: agentelier_conversation::ProjectChange,
    ) -> Result<(), RuntimeFailure> {
        self.send(Command::ChangeProject(id, change))
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
                    && control.matches_turn(input.session, input.turn.0)
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
                && control.matches_turn(session, turn)
                && !control.finished.is_cancelled()
        }) else {
            return Ok(false);
        };
        control.cancel.cancel();
        Ok(true)
    }
    fn send(&self, command: Command) -> Result<(), RuntimeFailure> {
        if self
            .active_model()?
            .as_ref()
            .is_some_and(|model| !model.finished.is_cancelled())
        {
            return Err(AccountFailure::Busy.into());
        }
        self.submit(command)
    }

    fn submit(&self, command: Command) -> Result<(), RuntimeFailure> {
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

fn model_source(
    account: &Account,
) -> Result<&agentelier_account::CodexIdentity, agentelier_conversation::ConversationFailure> {
    account
        .codex_identity()
        .ok_or_else(|| agentelier_conversation::ConversationFailure::new("请先使用 ChatGPT 登录"))
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
