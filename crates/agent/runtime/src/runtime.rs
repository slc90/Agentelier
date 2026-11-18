use agentelier_account::{
    Account, AccountCommand, AccountEventKind, AccountFailure, CredentialClient,
};
use agentelier_conversation::SessionId;
use agentelier_conversation::{
    Conversation, ConversationEvent, ConversationFailure, ConversationRequest, MAX_PENDING_STEERS,
    RunnerEvent, SessionCommand, SteerInput,
};
use agentelier_llm_model::{ModelFailure, ModelRequest, ModelResult};
use agentelier_memory::Memory;
mod creation;
mod deleting;
mod loading;
mod model_task;
mod recording;
mod renaming;
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
    McpConfig(io::Result<agentelier_mcp::ConfigSnapshot>),
    McpStatus(Vec<agentelier_mcp::ConnectionStatus>),
    McpMaterial(Result<agentelier_mcp::MaterialReply, String>),
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
enum Command {
    McpMaterial(u64, agentelier_mcp::MaterialRequest),
    CancelMcpMaterial(u64),
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
        operations: crate::OperationSequence,
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
                        let store = agentelier_session_store::SessionStore::start(data_root.clone()).map_err(RuntimeFailure::Store)?;
                        let memory_root = data_root.clone();
                        let memory = tokio::task::spawn_blocking(move || Memory::open(&memory_root))
                            .await.map_err(io::Error::other).and_then(|result| result);
                        match &memory {
                            Ok(_) => debug!(path = %data_root.display(), "记忆存储已打开"),
                            Err(failure) => error!(path = %data_root.display(), error = %failure, "记忆存储不可用"),
                        }
                        let mut mcp_settings = agentelier_mcp::ConfigStore::open(&data_root);
                        let mut material: Option<PendingMaterial> = None;
                        let mut mcp_connections = agentelier_mcp::Connections::default();
                        if let Ok(servers) = mcp_settings.servers() {
                            mcp_connections.configure(servers);
                        }
                        let mut creating: Option<creation::Creation> = None;
                        let mut loading: Option<loading::Loading> = None;
                        let mut catalog = Some(store.client().scan().map_err(RuntimeFailure::Store)?);
                        let mut startup_catalog = true;
                        let mut records = recording::Records::new();
                        let mut pending_queue = false;
                        let mut deleting: Option<deleting::Deleting> = None;
                        let mut renaming: Option<renaming::Renaming> = None;
                        let mut suspending = 0usize;
                        let mut finalizing = false;
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
                                    reply = async {
                                        match &mut material {
                                            Some(job) => (&mut job.task).await.unwrap_or_else(|_| Err("MCP 材料请求任务异常结束".into())),
                                            None => std::future::pending().await,
                                        }
                                    } => Command::McpMaterialFinished(reply),
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
                                    result = async { match &mut creating {
                                        Some(pending) => pending.wait().await,
                                        None => std::future::pending().await,
                                    }} => Command::Created(result),
                                    result = async { match &mut model {
                                        Some(task) => (&mut task.handle).await,
                                        None => std::future::pending().await,
                                    }} => Command::ModelFinished(result),
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
                                        emit(&publish, id, AgentEventKind::Conversation(pending.complete(result, &mut conversation)))?;
                                    }
                                    Command::Scan => {
                                        if catalog.is_none() {
                                            match store.client().scan() {
                                                Ok(reply) => catalog = Some(reply),
                                                Err(failure) => emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::ArchiveFailed(failure.to_string())))?,
                                            }
                                        }
                                    }
                                    Command::Scanned(result) => {
                                        catalog = None;
                                        match result {
                                            Ok(catalog) => {
                                                let entries = catalog.entries;
                                                let first = entries.first().map(|entry| entry.summary.session);
                                                emit(&publish, 0, AgentEventKind::Conversation(ConversationEvent::Archives(entries)))?;
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
                                        let event = pending.complete(*result, &operations, &mut conversation, &mcp_connections);
                                        let opened = matches!(&event, ConversationEvent::Opened(_));
                                        emit(&publish, id, AgentEventKind::Conversation(event))?;
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
                                        if model.is_none() && creating.is_none() && loading.is_none() && deleting.is_none() && suspending == 0 && !pending_queue && !finalizing && !runtime_shutdown.is_cancelled() {
                                            model_task::start_queued(&mut conversation,&mut model,&mut progress_events,&owner_model,&owner_credentials,&store,&publish,&data_root.join("skills"))?;
                                        }
                                    }
                                    Command::Created(result) => {
                                        let pending = creating.take().ok_or(RuntimeFailure::ModelState)?;
                                        let id = pending.id;
                                        let event = pending.complete(result, &mut conversation);
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
                                        debug!(operation_id = task.id, "模型操作已真实结束并回收");
                                        drain_progress(&mut conversation, task.id, &mut progress_events, &publish)?;
                                        let event = complete(&mut conversation, task.id, task.chat, result)?;
                                        if task.chat {
                                            let (state, history) = conversation.archive().ok_or(RuntimeFailure::ModelState)?;
                                            let session = state.session;
                                            recording::submit(recording::Pending::Finished { id: task.id, session, control: task.control }, agentelier_conversation::RecordEvent::Finished { state, history }, &store, &mut records);
                                            finalizing = true;
                                        } else {
                                            task.control.finished.cancel();
                                            capture_delivery(emit(&publish, task.id, event), &mut delivery_failure);
                                        }
                                        if !finalizing && !pending_queue && suspending == 0 && delivery_failure.is_none() && !runtime_shutdown.is_cancelled() {
                                            model_task::start_queued(&mut conversation,&mut model,&mut progress_events,&owner_model,&owner_credentials,&store,&publish,&data_root.join("skills"))?;
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
                                        if matches!(command, SessionCommand::Resume { .. }) && current_subject.is_none() {
                                            let id = command.id();
                                            emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Queue(agentelier_conversation::QueueEvent { id, view: conversation.current(), decision: agentelier_conversation::QueueDecision::Rejected("请先登录当前账号，再明确继续队列".into()) })))?;
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
                                        if model.is_some() || creating.is_some() || loading.is_some() || deleting.is_some() || suspending != 0 || finalizing || pending_queue {
                                            control.finished.cancel();
                                            emit(&publish,id,AgentEventKind::Conversation(ConversationEvent::Rejected("排队任务已经开始，当前发送未接纳，草稿保留".into())))?;
                                            continue;
                                        }
                                        match request {
                                            ConversationRequest::Delete { session } => {
                                                match if renaming.is_some() { Err(io::Error::other("名称写入尚未完成，请稍后删除")) } else { deleting::Deleting::start(id, control.clone(), session, &store) } {
                                                    Ok(pending) => { *owner_model.lock().map_err(|_| RuntimeFailure::ModelState)? = Some(control); deleting = Some(pending); }
                                                    Err(failure) => { control.finished.cancel(); emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected(failure.to_string())))?; }
                                                }
                                            }
                                            ConversationRequest::Open { session } => {
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
                                            request @ (ConversationRequest::Create(_) | ConversationRequest::Reset { .. }) => {
                                                match creation::Creation::start(id, control.clone(), request, &conversation, &store, &mcp_connections) {
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
                                                if current_subject.is_none() { control.finished.cancel(); emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected("请先登录当前账号后再开始任务".into())))?; continue; }
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
                                                            TaskRequest::Chat(Box::new(request), sender, steers, data_root.join("skills")),
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
                                                    &store,
                                                    &publish,
                                                )
                                                .await,
                                                &mut delivery_failure,
                                            );
                                            while let Ok(queued) = incoming.try_recv() {
                                                match queued {
                                                    Command::McpMaterial(id, _) => {
                                                        emit(&publish, id, AgentEventKind::McpMaterial(Err("账号操作开始前尚未接纳 MCP 材料操作，请重新提交".into())))?;
                                                    }
                                                    Command::CancelMcpMaterial(id) => {
                                                        if let Some(job) = &material && job.id == id {
                                                            job.cancel.cancel();
                                                        }
                                                    }
                                                    Command::McpConfig(id, _) => {
                                                        emit(&publish, id, AgentEventKind::McpConfig(Err(io::Error::other("账号操作开始前尚未接纳 MCP 配置操作，请重新提交"))))?;
                                                    }
                                                    Command::Conversation { id, control, request: ConversationRequest::Delete { .. }, .. } => {
                                                        control.finished.cancel();
                                                        emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Rejected("账号操作开始前尚未接纳删除，请重新提交".into())))?;
                                                    }
                                                    Command::Rename { id, session, .. } => emit(&publish, id, AgentEventKind::Conversation(ConversationEvent::Renamed { session, result: Err("账号操作开始前尚未接纳名称写入，请重新提交".into()) }))?,
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
                                            if current_subject != subject {
                                                if model.is_some() { stop_model(&mut model, &mut progress_events, &mut conversation, &store, &publish).await?; }
                                                if let Some(event) = conversation.suspend_queue()? {
                                                    let session = event.state().session;
                                                    recording::submit(recording::Pending::Suspended { session }, event, &store, &mut records);
                                                    suspending = suspending.checked_add(1).ok_or(RuntimeFailure::ModelState)?;
                                                }
                                            }
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
                                &store,
                                &publish,
                            )
                            .await,
                            &mut delivery_failure,
                        );
                        if let Some(job) = material {
                            job.cancel.cancel();
                            if job.task.await.is_err() {
                                error!(operation_id = job.id, "关闭时 MCP 材料请求任务异常结束");
                            }
                        }
                        let mcp_finished = mcp_connections.shutdown().await;
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
                        while let Some(result) = records.join_next().await {
                            match result {
                                Ok((pending, result)) => capture_delivery(recording::apply(pending, result, &mut conversation, &publish).map(|_| ()), &mut delivery_failure),
                                Err(_) => { delivery_failure = delivery_failure.or(Some(RuntimeFailure::ModelState)); }
                            }
                        }
                        if let Some(mut pending) = renaming { let result = pending.wait().await; let id = pending.id; capture_delivery(emit(&publish, id, AgentEventKind::Conversation(pending.complete(result))), &mut delivery_failure); }
                        if let Some(mut pending) = deleting { let result = pending.wait().await; let id = pending.id; capture_delivery(emit(&publish, id, AgentEventKind::Conversation(pending.complete(result, &mut conversation))), &mut delivery_failure); }
                        let stored = store.finish().await.map_err(RuntimeFailure::Store);
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
                        mcp_finished.map_err(RuntimeFailure::Mcp)?;
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
            active: None,
            credentials,
            shutdown,
            sign_out_commands,
            model: shared_model,
            session_commands,
        })
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
