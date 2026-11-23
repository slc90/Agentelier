use agentelier_account_bridge::AccountEventKind;
use agentelier_agent_runtime::{AgentEventKind, AgentRuntime, DeliveryFailure};
use agentelier_conversation_bridge::SessionId;
use agentelier_llm_model_bridge::ModelEventKind;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::{error::Error, fmt};

const RESULT_CAPACITY: usize = 32;
type Notify = Box<dyn Fn() -> bool + Send + Sync>;

pub struct BridgeEvent {
    pub operation_id: u64,
    pub kind: BridgeEventKind,
}
pub enum BridgeEventKind {
    Account(AccountEventKind),
    Model(ModelEventKind),
    Conversation(Box<agentelier_conversation_bridge::ConversationEvent>),
    McpConfig(Result<crate::ConfigSnapshot, String>),
    McpStatus(Vec<crate::ConnectionStatus>),
    Memory(Result<crate::MemoryReply, String>),
    ImageAttachment(Result<crate::ImageAttachmentReply, String>),
    McpMaterial(Result<crate::MaterialReply, String>),
}
#[derive(Clone, Debug)]
pub struct BridgeFailure(String);
impl fmt::Display for BridgeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for BridgeFailure {}

struct WakeGate(Mutex<Option<Notify>>);
impl WakeGate {
    fn signal(&self) -> Result<(), DeliveryFailure> {
        let notify = self.0.lock().map_err(|_| DeliveryFailure::Wake)?;
        if notify.as_ref().is_some_and(|notify| !notify()) {
            return Err(DeliveryFailure::Wake);
        }
        Ok(())
    }
    fn disable(&self) -> Result<(), BridgeFailure> {
        self.0
            .lock()
            .map_err(|_| BridgeFailure("认证唤醒状态锁已损坏".into()))?
            .take();
        Ok(())
    }
}
enum QueueRequest {
    Enqueue,
    Remove,
    Resume,
}

pub struct Connection {
    runtime: AgentRuntime,
    events: mpsc::Receiver<BridgeEvent>,
    progress: Arc<Mutex<Option<BridgeEvent>>>,
    mcp_status: Arc<Mutex<Option<BridgeEvent>>>,
    global_tasks: Arc<Mutex<Option<BridgeEvent>>>,
    deferred: Option<BridgeEvent>,
    wake: Arc<WakeGate>,
    operations: agentelier_agent_runtime::OperationSequence,
    pending: Option<u64>,
    stopping: bool,
    failure: Option<BridgeFailure>,
    model: Option<u64>,
    controls: HashMap<u64, u64>,
    renaming: HashMap<u64, SessionId>,
    deleting: HashMap<u64, SessionId>,
    queue_controls: HashMap<u64, QueueRequest>,
    reserved_queue: HashSet<u64>,
    pending_requests: HashSet<u64>,
    pending_mcp: HashSet<u64>,
    pending_memory: HashSet<u64>,
    pending_projects: HashSet<u64>,
    pending_session_models: HashMap<u64, SessionId>,
    pending_images: HashSet<u64>,
    pending_audio: HashMap<u64, SessionId>,
}
impl Connection {
    pub fn start(
        environment: crate::DataEnvironment,
        notify: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, BridgeFailure> {
        let (sender, events) = mpsc::sync_channel(RESULT_CAPACITY);
        let progress = Arc::new(Mutex::new(None));
        let delivery_progress = progress.clone();
        let mcp_status = Arc::new(Mutex::new(None));
        let delivery_mcp_status = mcp_status.clone();
        let global_tasks = Arc::new(Mutex::new(None));
        let delivery_global_tasks = global_tasks.clone();
        let wake = Arc::new(WakeGate(Mutex::new(Some(Box::new(notify)))));
        let delivery_wake = wake.clone();
        let finished_wake = wake.clone();
        let operations = agentelier_agent_runtime::OperationSequence::default();
        let runtime = AgentRuntime::start(
            environment,
            operations.clone(),
            move |event| {
                let event = BridgeEvent {
                    operation_id: event.operation_id,
                    kind: match event.kind {
                        AgentEventKind::Account(kind) => {
                            BridgeEventKind::Account(agentelier_account_bridge::convert(kind))
                        }
                        AgentEventKind::ModelCompleted(result) => BridgeEventKind::Model(
                            agentelier_llm_model_bridge::convert(Some(result)),
                        ),
                        AgentEventKind::ModelCancelled => {
                            BridgeEventKind::Model(agentelier_llm_model_bridge::convert(None))
                        }
                        AgentEventKind::Memory(reply) => BridgeEventKind::Memory(reply),
                        AgentEventKind::ImageAttachment(reply) => BridgeEventKind::ImageAttachment(reply),
                        AgentEventKind::McpMaterial(reply) => BridgeEventKind::McpMaterial(reply),
                        AgentEventKind::McpStatus(status) => BridgeEventKind::McpStatus(status),
                        AgentEventKind::McpConfig(result) => {
                            BridgeEventKind::McpConfig(result.map_err(|error| error.to_string()))
                        }
                        AgentEventKind::Conversation(kind) => BridgeEventKind::Conversation(Box::new(
                            agentelier_conversation_bridge::convert(kind),
                        )),
                    },
                };
                if matches!(event.kind, BridgeEventKind::McpStatus(_)) {
                    *delivery_mcp_status.lock().map_err(|_| DeliveryFailure::Closed)? = Some(event);
                } else if matches!(&event.kind, BridgeEventKind::Conversation(kind)
                    if matches!(kind.as_ref(), agentelier_conversation_bridge::ConversationEvent::GlobalTasks(_)))
                {
                    *delivery_global_tasks.lock().map_err(|_| DeliveryFailure::Closed)? = Some(event);
                } else if matches!(&event.kind, BridgeEventKind::Conversation(kind) if kind.is_progress())
                {
                    *delivery_progress
                        .lock()
                        .map_err(|_| DeliveryFailure::Closed)? = Some(event);
                } else {
                    sender.try_send(event).map_err(|error| match error {
                        mpsc::TrySendError::Full(_) => DeliveryFailure::Full,
                        mpsc::TrySendError::Disconnected(_) => DeliveryFailure::Closed,
                    })?;
                }
                delivery_wake.signal()
            },
            move || finished_wake.signal(),
        )
        .map_err(|error| BridgeFailure(error.to_string()))?;
        Ok(Self {
            runtime,
            events,
            progress,
            mcp_status,
            global_tasks,
            deferred: None,
            wake,
            operations,
            pending: None,
            stopping: false,
            failure: None,
            model: None,
            controls: HashMap::new(),
            renaming: HashMap::new(),
            deleting: HashMap::new(),
            queue_controls: HashMap::new(),
            reserved_queue: HashSet::new(),
            pending_requests: HashSet::new(),
            pending_mcp: HashSet::new(),
            pending_memory: HashSet::new(),
            pending_projects: HashSet::new(),
            pending_session_models: HashMap::new(),
            pending_images: HashSet::new(),
            pending_audio: HashMap::new(),
        })
    }
    pub fn request_memory(&mut self, request: crate::MemoryRequest) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("执行连接已经关闭".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .request_memory(id, request)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.pending_memory.insert(id);
        Ok(id)
    }

    pub fn request_image_attachment(
        &mut self,
        session: SessionId,
        call: String,
        action: crate::ImageAttachmentAction,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("执行连接已经关闭".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .request_image_attachment(id, session, call, action)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.pending_images.insert(id);
        Ok(id)
    }

    pub fn change_audio_draft(
        &mut self,
        session: SessionId,
        path: Option<PathBuf>,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("执行连接已经关闭".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .change_audio_draft(id, session, path)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.pending_audio.insert(id, session);
        Ok(id)
    }

    pub fn transcribe_audio(
        &mut self,
        session: SessionId,
        audio: SessionId,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("执行连接已经关闭".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .transcribe_audio(id, session, audio)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.pending_audio.insert(id, session);
        Ok(id)
    }

    pub fn cancel_transcription(&self, session: SessionId, id: u64) -> Result<bool, BridgeFailure> {
        if self.pending_audio.get(&id) != Some(&session) {
            return Ok(false);
        }
        self.runtime
            .cancel_transcription(id, session)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        Ok(true)
    }

    pub fn mcp_config(&mut self, change: crate::ConfigChange) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("执行连接已经关闭".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.runtime
            .mcp_config(id, change)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.pending_mcp.insert(id);
        Ok(id)
    }

    pub fn mcp_material(&mut self, request: crate::MaterialRequest) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("执行连接已经关闭".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.runtime
            .mcp_material(id, request)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.pending_mcp.insert(id);
        Ok(id)
    }

    pub fn cancel_mcp_material(&mut self, id: u64) -> Result<(), BridgeFailure> {
        if self.pending_mcp.contains(&id) {
            self.runtime
                .cancel_mcp_material(id)
                .map_err(|error| BridgeFailure(error.to_string()))?;
        }
        Ok(())
    }

    fn allocate(&self) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("认证运行已停止".into()));
        }
        if self.pending.is_some() || self.model.is_some() {
            return Err(BridgeFailure("已有账号操作正在进行".into()));
        }
        self.operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))
    }
    pub fn begin_login(&mut self) -> Result<u64, BridgeFailure> {
        let id = self.allocate()?;
        self.runtime
            .begin_login(id)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.pending = Some(id);
        Ok(id)
    }
    pub fn cancel_login(&self, id: u64) -> Result<(), BridgeFailure> {
        if self.pending == Some(id) {
            self.runtime
                .cancel_login(id)
                .map_err(|error| BridgeFailure(error.to_string()))?;
        }
        Ok(())
    }
    pub fn logout(&mut self) -> Result<u64, BridgeFailure> {
        let id = self.allocate()?;
        self.runtime
            .logout(id)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.pending = Some(id);
        Ok(id)
    }
    pub fn request_model(
        &mut self,
        request: agentelier_llm_model_bridge::ModelRequest,
    ) -> Result<u64, BridgeFailure> {
        let id = self.allocate()?;
        self.runtime
            .request_model(id, agentelier_llm_model_bridge::request(request))
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.model = Some(id);
        self.pending_requests.insert(id);
        Ok(id)
    }
    pub fn request_conversation(
        &mut self,
        request: agentelier_conversation_bridge::ConversationRequest,
    ) -> Result<u64, BridgeFailure> {
        let id = if matches!(
            request,
            agentelier_conversation_bridge::ConversationRequest::Open { .. }
                | agentelier_conversation_bridge::ConversationRequest::Delete { .. }
                | agentelier_conversation_bridge::ConversationRequest::SetWorkingDirectory { .. }
        ) {
            if self.stopping || self.failure.is_some() || self.model.is_some() {
                return Err(BridgeFailure(
                    "当前任务结束并回收后才能切换、删除会话或变更目录".into(),
                ));
            }
            self.operations
                .allocate()
                .map_err(|failure| BridgeFailure(failure.to_string()))?
        } else {
            self.allocate()?
        };
        let deleting = match &request {
            agentelier_conversation_bridge::ConversationRequest::Delete { session } => {
                Some(*session)
            }
            _ => None,
        };
        self.runtime
            .request_conversation(id, agentelier_conversation_bridge::request(request))
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        if let Some(session) = deleting {
            self.deleting.insert(id, session);
        }
        self.model = Some(id);
        self.pending_requests.insert(id);
        Ok(id)
    }
    pub fn refresh_archives(&self) -> Result<(), BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("执行连接已停止".into()));
        }
        self.runtime
            .refresh_archives()
            .map_err(|failure| BridgeFailure(failure.to_string()))
    }
    pub fn change_session_model(
        &mut self,
        session: SessionId,
        slug: String,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() || !self.pending_session_models.is_empty() {
            return Err(BridgeFailure("当前执行连接不能接纳模型变更".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .change_session_model(id, session, slug)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.pending_session_models.insert(id, session);
        Ok(id)
    }
    pub fn steer_conversation(
        &mut self,
        session: SessionId,
        turn: u64,
        operation: u64,
        text: String,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() || self.model != Some(operation) {
            return Err(BridgeFailure("当前任务不能接纳补充输入".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .steer_conversation(agentelier_conversation_bridge::steer_input(
                id, session, turn, operation, text,
            ))
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.controls.insert(id, operation);
        Ok(id)
    }

    pub fn rename_session(
        &mut self,
        session: SessionId,
        title: String,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() || !self.renaming.is_empty() {
            return Err(BridgeFailure("当前不能接纳名称操作".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .rename_session(id, session, title)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.renaming.insert(id, session);
        Ok(id)
    }
    pub fn change_project(
        &mut self,
        change: agentelier_conversation_bridge::ProjectChange,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("当前执行连接不能接纳项目操作".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .change_project(id, change)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.pending_projects.insert(id);
        Ok(id)
    }
    pub fn request_session(
        &mut self,
        action: agentelier_conversation_bridge::SessionAction,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() || self.pending.is_some() {
            return Err(BridgeFailure("当前执行连接不能接纳队列操作".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        let kind = match &action {
            agentelier_conversation_bridge::SessionAction::Enqueue { .. } => QueueRequest::Enqueue,
            agentelier_conversation_bridge::SessionAction::Remove { .. } => QueueRequest::Remove,
            agentelier_conversation_bridge::SessionAction::Resume { .. } => QueueRequest::Resume,
        };
        self.runtime
            .request_session(agentelier_conversation_bridge::session_command(id, action))
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        if matches!(kind, QueueRequest::Enqueue) {
            self.reserved_queue.insert(id);
        }
        self.queue_controls.insert(id, kind);
        Ok(id)
    }

    pub fn request_task_action(
        &mut self,
        session: SessionId,
        action: agentelier_conversation_bridge::TaskAction,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() || self.pending.is_some() {
            return Err(BridgeFailure("当前执行连接不能接纳任务控制".into()));
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .request_task_action(id, session, action)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        Ok(id)
    }
    pub fn model_busy(&self) -> bool {
        self.model.is_some()
    }

    pub fn cancel_conversation(
        &mut self,
        session: SessionId,
        turn: u64,
        operation: u64,
    ) -> Result<bool, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("聊天运行正在结束".into()));
        }
        if self.model != Some(operation) {
            return Ok(false);
        }
        self.runtime
            .cancel_conversation(session, turn, operation)
            .map_err(|failure| BridgeFailure(failure.to_string()))
    }
    fn take_progress(&self) -> Result<Option<BridgeEvent>, BridgeFailure> {
        Ok(self
            .progress
            .lock()
            .map_err(|_| BridgeFailure("聊天增量状态锁已损坏".into()))?
            .take())
    }
    pub fn try_receive(&mut self) -> Result<Option<BridgeEvent>, BridgeFailure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if !self.stopping && self.runtime.is_finished() {
            let failure = match self.runtime.finish() {
                Err(error) => BridgeFailure(error.to_string()),
                Ok(()) => BridgeFailure("认证运行意外停止".into()),
            };
            self.failure = Some(failure.clone());
            return Err(failure);
        }
        let was_deferred = self.deferred.is_some();
        let received = self
            .deferred
            .take()
            .map(Ok)
            .unwrap_or_else(|| self.events.try_recv());
        let received = match received {
            Err(mpsc::TryRecvError::Empty) => match self.take_progress()? {
                Some(event) => Ok(event),
                None => Err(mpsc::TryRecvError::Empty),
            },
            other => other,
        };
        let received = match received {
            Err(mpsc::TryRecvError::Empty) => self
                .mcp_status
                .lock()
                .map_err(|_| BridgeFailure("MCP 连接状态锁已损坏".into()))?
                .take()
                .ok_or(mpsc::TryRecvError::Empty),
            other => other,
        };
        let received = match received {
            Err(mpsc::TryRecvError::Empty) => self
                .global_tasks
                .lock()
                .map_err(|_| BridgeFailure("全局任务状态锁已损坏".into()))?
                .take()
                .ok_or(mpsc::TryRecvError::Empty),
            other => other,
        };
        match received {
            Ok(event) => {
                if let BridgeEventKind::Conversation(kind) = &event.kind {
                    if let agentelier_conversation_bridge::ConversationEvent::AudioDraft {
                        session,
                        ..
                    }
                    | agentelier_conversation_bridge::ConversationEvent::AudioTranscription {
                        session,
                        ..
                    } = kind.as_ref()
                    {
                        if self.pending_audio.remove(&event.operation_id) != Some(*session) {
                            return self.protocol_failure();
                        }
                        return Ok(Some(event));
                    }
                    let kind = kind.as_ref();
                    if matches!(
                        kind,
                        agentelier_conversation_bridge::ConversationEvent::GlobalTasks(_)
                            | agentelier_conversation_bridge::ConversationEvent::TaskNotification(
                                _
                            )
                    ) {
                        if event.operation_id != 0 {
                            return self.protocol_failure();
                        }
                        return Ok(Some(event));
                    }
                    if matches!(
                        kind,
                        agentelier_conversation_bridge::ConversationEvent::ProjectSaved(_)
                    ) {
                        if !self.pending_projects.remove(&event.operation_id) {
                            return self.protocol_failure();
                        }
                        return Ok(Some(event));
                    }
                    if matches!(
                        kind,
                        agentelier_conversation_bridge::ConversationEvent::Projects(_)
                    ) {
                        if event.operation_id != 0 {
                            return self.protocol_failure();
                        }
                        return Ok(Some(event));
                    }
                    if let agentelier_conversation_bridge::ConversationEvent::Renamed {
                        session,
                        ..
                    } = kind
                    {
                        if self.renaming.remove(&event.operation_id) != Some(*session) {
                            return self.protocol_failure();
                        }
                        return Ok(Some(event));
                    }
                    if let agentelier_conversation_bridge::ConversationEvent::SessionModelSaved {
                        session,
                        ..
                    } = kind
                    {
                        if self.pending_session_models.remove(&event.operation_id) != Some(*session)
                        {
                            return self.protocol_failure();
                        }
                        return Ok(Some(event));
                    }
                    if let agentelier_conversation_bridge::ConversationEvent::Deleted {
                        session,
                        was_open,
                        ..
                    } = kind
                    {
                        if self.deleting.remove(&event.operation_id) != Some(*session) {
                            return self.protocol_failure();
                        }
                        self.pending_requests.remove(&event.operation_id);
                        if self.model == Some(event.operation_id) {
                            self.model = None;
                        }
                        if *was_open {
                            self.reserved_queue.clear();
                        }
                        return Ok(Some(event));
                    }
                    if matches!(
                        kind,
                        agentelier_conversation_bridge::ConversationEvent::Rejected(_)
                    ) && self.deleting.remove(&event.operation_id).is_some()
                    {
                        self.pending_requests.remove(&event.operation_id);
                        if self.model == Some(event.operation_id) {
                            self.model = None;
                        }
                        return Ok(Some(event));
                    }
                }
                if matches!(event.kind, BridgeEventKind::Memory(_)) {
                    if !self.pending_memory.remove(&event.operation_id) {
                        return self.protocol_failure();
                    }
                    return Ok(Some(event));
                }
                if matches!(event.kind, BridgeEventKind::ImageAttachment(_)) {
                    if !self.pending_images.remove(&event.operation_id) {
                        return self.protocol_failure();
                    }
                    return Ok(Some(event));
                }
                if matches!(event.kind, BridgeEventKind::McpStatus(_)) {
                    return Ok(Some(event));
                }
                if matches!(
                    event.kind,
                    BridgeEventKind::McpConfig(_) | BridgeEventKind::McpMaterial(_)
                ) {
                    if !self.pending_mcp.remove(&event.operation_id) {
                        return self.protocol_failure();
                    }
                    return Ok(Some(event));
                }
                let kind = match &event.kind {
                    BridgeEventKind::McpStatus(_)
                    | BridgeEventKind::Memory(_)
                    | BridgeEventKind::ImageAttachment(_)
                    | BridgeEventKind::McpConfig(_)
                    | BridgeEventKind::McpMaterial(_) => {
                        return self.protocol_failure();
                    }
                    BridgeEventKind::Model(_) => {
                        if self.pending_requests.remove(&event.operation_id)
                            && self.model != Some(event.operation_id)
                        {
                            return Ok(Some(event));
                        }
                        if self.model != Some(event.operation_id) {
                            return self.protocol_failure();
                        }
                        self.model = None;
                        return Ok(Some(event));
                    }
                    BridgeEventKind::Conversation(kind) => {
                        let kind = kind.as_ref();
                        if matches!(
                            kind,
                            agentelier_conversation_bridge::ConversationEvent::TaskAction(_)
                        ) {
                            return Ok(Some(event));
                        }
                        if event.operation_id == 0 && matches!(kind, agentelier_conversation_bridge::ConversationEvent::Tasks(_) | agentelier_conversation_bridge::ConversationEvent::Processes(_) | agentelier_conversation_bridge::ConversationEvent::IdleUpdated(_) | agentelier_conversation_bridge::ConversationEvent::ArchiveCleanup(_) | agentelier_conversation_bridge::ConversationEvent::Archives(_) | agentelier_conversation_bridge::ConversationEvent::Opened(_) | agentelier_conversation_bridge::ConversationEvent::ArchiveFailed(_)) {
                            if let agentelier_conversation_bridge::ConversationEvent::Opened(view) = kind { self.reserved_queue.extend(view.queue.iter().map(|input| input.operation_id)); }
                            return Ok(Some(event));
                        }
                        if let agentelier_conversation_bridge::ConversationEvent::Queue(reply) =
                            kind
                        {
                            let Some(request) = self.queue_controls.remove(&reply.id) else {
                                return self.protocol_failure();
                            };
                            if reply.id != event.operation_id {
                                return self.protocol_failure();
                            }
                            match &reply.decision {
                                agentelier_conversation_bridge::QueueDecision::Rejected(_)
                                    if matches!(request, QueueRequest::Enqueue) =>
                                {
                                    self.reserved_queue.remove(&reply.id);
                                }
                                agentelier_conversation_bridge::QueueDecision::Removed {
                                    operation,
                                } => {
                                    self.reserved_queue.remove(operation);
                                }
                                _ => {}
                            }
                            return Ok(Some(event));
                        }
                        if let agentelier_conversation_bridge::ConversationEvent::Started(_) = kind
                        {
                            if !self.reserved_queue.remove(&event.operation_id)
                                || self
                                    .model
                                    .is_some_and(|id| !self.pending_requests.contains(&id))
                            {
                                return self.protocol_failure();
                            }
                            self.model = Some(event.operation_id);
                            return Ok(Some(event));
                        }
                        if matches!(
                            kind,
                            agentelier_conversation_bridge::ConversationEvent::Rejected(_)
                        ) && self.pending_requests.remove(&event.operation_id)
                            && self.model != Some(event.operation_id)
                        {
                            return Ok(Some(event));
                        }

                        if let agentelier_conversation_bridge::ConversationEvent::InputReceipt(
                            receipt,
                        ) = kind
                        {
                            if !self.controls.contains_key(&receipt.id)
                                || self.controls.get(&receipt.id) != self.model.as_ref()
                                || receipt.id != event.operation_id
                            {
                                return self.protocol_failure();
                            }
                            if !matches!(
                                receipt.decision,
                                agentelier_conversation_bridge::InputDecision::Accepted
                            ) {
                                self.controls.remove(&receipt.id);
                            }
                            return Ok(Some(event));
                        }
                        if self.model != Some(event.operation_id) {
                            return self.protocol_failure();
                        }
                        if matches!(
                            kind,
                            agentelier_conversation_bridge::ConversationEvent::Accepted(_)
                                | agentelier_conversation_bridge::ConversationEvent::Created(_)
                                | agentelier_conversation_bridge::ConversationEvent::WorkingDirectorySaved(_)
                                | agentelier_conversation_bridge::ConversationEvent::Opened(_)
                                | agentelier_conversation_bridge::ConversationEvent::ArchiveFailed(
                                    _
                                )
                        ) {
                            self.pending_requests.remove(&event.operation_id);
                        }
                        if matches!(
                            kind,
                            agentelier_conversation_bridge::ConversationEvent::Created(_)
                        ) {
                            self.reserved_queue.clear();
                        }
                        if let agentelier_conversation_bridge::ConversationEvent::Opened(view) =
                            kind
                        {
                            self.reserved_queue.clear();
                            self.reserved_queue
                                .extend(view.queue.iter().map(|input| input.operation_id));
                        }
                        if let agentelier_conversation_bridge::ConversationEvent::Finished(view) =
                            kind
                            && !was_deferred
                        {
                            let snapshot = BridgeEvent {
                                operation_id: event.operation_id,
                                kind: BridgeEventKind::Conversation(Box::new(
                                    agentelier_conversation_bridge::ConversationEvent::Updated(
                                        view.clone(),
                                    ),
                                )),
                            };
                            let mut progress = self
                                .progress
                                .lock()
                                .map_err(|_| BridgeFailure("聊天增量状态锁已损坏".into()))?;
                            if progress
                                .as_ref()
                                .is_some_and(|progress| progress.operation_id == event.operation_id)
                            {
                                *progress = None;
                            }
                            self.deferred = Some(event);
                            return Ok(Some(snapshot));
                        }
                        if kind.is_terminal() {
                            self.controls
                                .retain(|_, operation| *operation != event.operation_id);
                            self.model = None;
                        }
                        return Ok(Some(event));
                    }
                    BridgeEventKind::Account(kind) => kind,
                };
                if self.pending == Some(event.operation_id)
                    && matches!(
                        kind,
                        AccountEventKind::SourceChanged(_)
                            | AccountEventKind::LoginCancelled
                            | AccountEventKind::LogoutFinished { .. }
                    )
                {
                    self.pending = None;
                }
                Ok(Some(event))
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) if self.stopping => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                let failure = BridgeFailure("认证结果通道已关闭".into());
                self.failure = Some(failure.clone());
                Err(failure)
            }
        }
    }
    fn protocol_failure(&mut self) -> Result<Option<BridgeEvent>, BridgeFailure> {
        let failure = BridgeFailure("账号结果不属于当前操作".into());
        self.failure = Some(failure.clone());
        Err(failure)
    }
    pub fn begin_shutdown(&mut self) -> Result<(), BridgeFailure> {
        self.stopping = true;
        let result = self.wake.disable();
        self.runtime.begin_shutdown();
        result
    }
    pub fn is_finished(&self) -> bool {
        self.runtime.is_finished()
    }
    pub fn finish(&mut self) -> Result<(), BridgeFailure> {
        let stop = self.begin_shutdown();
        let finish = self
            .runtime
            .finish()
            .map_err(|error| BridgeFailure(error.to_string()));
        stop?;
        finish?;
        if let Some(failure) = self.failure.take() {
            return Err(failure);
        }
        Ok(())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Err(failure) = self.finish() {
            tracing::error!(error = %failure, "未显式收尾的业务桥接在释放时结束失败");
        }
    }
}
