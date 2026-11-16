use agentelier_account_bridge::{AccountEventKind, SessionActivity};
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
    Conversation(agentelier_conversation_bridge::ConversationEvent),
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
#[derive(Clone, Copy)]
enum Phase {
    Restore,
    Login,
    SaveFailed,
    Saving,
}
struct Pending {
    id: u64,
    phase: Phase,
    cancelling: bool,
}
enum QueueRequest {
    Enqueue,
    Remove,
    Resume,
}

struct PendingSignOut {
    id: u64,
    failed: bool,
}

pub struct Connection {
    runtime: AgentRuntime,
    events: mpsc::Receiver<BridgeEvent>,
    progress: Arc<Mutex<Option<BridgeEvent>>>,
    deferred: Option<BridgeEvent>,
    wake: Arc<WakeGate>,
    operations: agentelier_agent_runtime::OperationSequence,
    pending: Option<Pending>,
    stopping: bool,
    failure: Option<BridgeFailure>,
    session_activity: SessionActivity,
    sign_out: Option<PendingSignOut>,
    model: Option<u64>,
    controls: HashMap<u64, u64>,
    queue_controls: HashMap<u64, QueueRequest>,
    reserved_queue: HashSet<u64>,
    pending_requests: HashSet<u64>,
}
impl Connection {
    pub fn start(
        path: PathBuf,
        notify: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, BridgeFailure> {
        let (sender, events) = mpsc::sync_channel(RESULT_CAPACITY);
        let progress = Arc::new(Mutex::new(None));
        let delivery_progress = progress.clone();
        let wake = Arc::new(WakeGate(Mutex::new(Some(Box::new(notify)))));
        let delivery_wake = wake.clone();
        let finished_wake = wake.clone();
        let operations = agentelier_agent_runtime::OperationSequence::default();
        let runtime = AgentRuntime::start(
            path,
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
                        AgentEventKind::Conversation(kind) => BridgeEventKind::Conversation(
                            agentelier_conversation_bridge::convert(kind),
                        ),
                    },
                };
                if matches!(&event.kind, BridgeEventKind::Conversation(kind) if kind.is_progress())
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
            deferred: None,
            wake,
            operations,
            pending: None,
            stopping: false,
            failure: None,
            session_activity: SessionActivity::Idle,
            sign_out: None,
            model: None,
            controls: HashMap::new(),
            queue_controls: HashMap::new(),
            reserved_queue: HashSet::new(),
            pending_requests: HashSet::new(),
        })
    }
    fn allocate(&self) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("认证运行已停止".into()));
        }
        if self.pending.is_some()
            || self.model.is_some()
            || self.sign_out.is_some()
            || self.session_activity != SessionActivity::Idle
        {
            return Err(BridgeFailure("已有账号操作正在进行".into()));
        }
        self.operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))
    }
    pub fn restore(&mut self) -> Result<u64, BridgeFailure> {
        let id = self.allocate()?;
        self.runtime
            .restore(id)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.pending = Some(Pending {
            id,
            phase: Phase::Restore,
            cancelling: false,
        });
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
        ) {
            if self.stopping
                || self.failure.is_some()
                || self.model.is_some()
                || self.sign_out.is_some()
            {
                return Err(BridgeFailure("当前执行尚未结束，不能打开另一会话".into()));
            }
            self.operations
                .allocate()
                .map_err(|failure| BridgeFailure(failure.to_string()))?
        } else {
            self.allocate()?
        };
        self.runtime
            .request_conversation(id, agentelier_conversation_bridge::request(request))
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
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
    pub fn steer_conversation(
        &mut self,
        session: SessionId,
        turn: u64,
        operation: u64,
        text: String,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping
            || self.failure.is_some()
            || self.sign_out.is_some()
            || self.model != Some(operation)
        {
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

    pub fn request_session(
        &mut self,
        action: agentelier_conversation_bridge::SessionAction,
    ) -> Result<u64, BridgeFailure> {
        if self.stopping
            || self.failure.is_some()
            || self.sign_out.is_some()
            || self.pending.is_some()
        {
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
    pub fn model_busy(&self) -> bool {
        self.model.is_some()
    }

    pub fn cancel_conversation(
        &mut self,
        session: SessionId,
        turn: u64,
        operation: u64,
    ) -> Result<bool, BridgeFailure> {
        if self.stopping || self.sign_out.is_some() || self.failure.is_some() {
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
    pub fn login(&mut self, consent: bool) -> Result<u64, BridgeFailure> {
        let id = self.allocate()?;
        self.runtime
            .login(id, consent)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.pending = Some(Pending {
            id,
            phase: Phase::Login,
            cancelling: false,
        });
        Ok(id)
    }
    pub fn check_session(&mut self) -> Result<(), BridgeFailure> {
        if self.stopping
            || self.sign_out.is_some()
            || self.failure.is_some()
            || self.pending.is_some()
            || self.model.is_some()
            || matches!(
                self.session_activity,
                SessionActivity::Checking | SessionActivity::Refreshing | SessionActivity::Saving
            )
        {
            return Err(BridgeFailure("认证运行当前不能接受会话检查".into()));
        }
        self.runtime
            .credentials()
            .request()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.session_activity = SessionActivity::Checking;
        Ok(())
    }
    pub fn retry_save(&mut self, id: u64) -> Result<(), BridgeFailure> {
        if self.stopping
            || self.sign_out.is_some()
            || !self.pending.as_ref().is_some_and(|pending| {
                pending.id == id
                    && matches!(pending.phase, Phase::SaveFailed)
                    && !pending.cancelling
            })
        {
            return Err(BridgeFailure("当前操作没有可重试的保存结果".into()));
        }
        self.runtime
            .retry_save(id)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        if let Some(pending) = &mut self.pending {
            pending.phase = Phase::Saving;
        }
        Ok(())
    }
    pub fn cancel(&mut self, id: u64) -> Result<(), BridgeFailure> {
        if self.stopping || self.sign_out.is_some() {
            return Err(BridgeFailure("认证运行正在结束".into()));
        }
        let pending = self
            .pending
            .as_mut()
            .filter(|pending| pending.id == id)
            .ok_or_else(|| BridgeFailure("登录操作已经结束".into()))?;
        if matches!(pending.phase, Phase::Restore) {
            return Err(BridgeFailure("账号读取不支持中途取消".into()));
        }
        if !pending.cancelling {
            self.runtime
                .cancel(id)
                .map_err(|error| BridgeFailure(error.to_string()))?;
            pending.cancelling = true;
        }
        Ok(())
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
        loop {
            let was_deferred = self.deferred.is_some();
            let received = self
                .deferred
                .take()
                .map(Ok)
                .unwrap_or_else(|| self.events.try_recv());
            let received = match received {
                Err(mpsc::TryRecvError::Empty) if self.sign_out.is_none() => {
                    match self.take_progress()? {
                        Some(event) => Ok(event),
                        None => Err(mpsc::TryRecvError::Empty),
                    }
                }
                other => other,
            };
            match received {
                Ok(event) => {
                    if let BridgeEventKind::Account(kind) = &event.kind
                        && matches!(
                            kind,
                            AccountEventKind::SignOutProgress(_)
                                | AccountEventKind::SignOutFailed(_)
                                | AccountEventKind::SignedOut { .. }
                        )
                    {
                        let Some(pending) = &mut self.sign_out else {
                            return self.protocol_failure();
                        };
                        if pending.id != event.operation_id {
                            return self.protocol_failure();
                        }
                        match kind {
                            AccountEventKind::SignOutFailed(_) => pending.failed = true,
                            AccountEventKind::SignedOut { .. } => {
                                self.sign_out = None;
                                self.pending = None;
                                self.model = None;
                                self.session_activity = SessionActivity::Idle;
                            }
                            _ => {}
                        }
                        return Ok(Some(event));
                    }
                    if self.sign_out.is_some() {
                        if matches!(
                            &event.kind,
                            BridgeEventKind::Account(
                                AccountEventKind::Completed(_) | AccountEventKind::Cancelled
                            )
                        ) && self
                            .pending
                            .as_ref()
                            .is_some_and(|pending| pending.id == event.operation_id)
                        {
                            self.pending = None;
                        }
                        continue;
                    }
                    let kind = match &event.kind {
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
                            if event.operation_id == 0 && matches!(kind, agentelier_conversation_bridge::ConversationEvent::Archives(_) | agentelier_conversation_bridge::ConversationEvent::Opened(_) | agentelier_conversation_bridge::ConversationEvent::ArchiveFailed(_)) {
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
                            if let agentelier_conversation_bridge::ConversationEvent::Started(_) =
                                kind
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
                                    | agentelier_conversation_bridge::ConversationEvent::Opened(_)
                                    | agentelier_conversation_bridge::ConversationEvent::ArchiveFailed(_)
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
                                    kind: BridgeEventKind::Conversation(
                                        agentelier_conversation_bridge::ConversationEvent::Updated(
                                            view.clone(),
                                        ),
                                    ),
                                };
                                let mut progress = self
                                    .progress
                                    .lock()
                                    .map_err(|_| BridgeFailure("聊天增量状态锁已损坏".into()))?;
                                if progress.as_ref().is_some_and(|progress| {
                                    progress.operation_id == event.operation_id
                                }) {
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
                    if let AccountEventKind::SessionStatus { activity, .. } = kind {
                        self.session_activity = *activity;
                        return Ok(Some(event));
                    }
                    let Some(pending) = &mut self.pending else {
                        return self.protocol_failure();
                    };
                    if event.operation_id != pending.id {
                        return self.protocol_failure();
                    }
                    match kind {
                        AccountEventKind::BrowserReady(_) if pending.cancelling => continue,
                        AccountEventKind::SaveFailed(_) => pending.phase = Phase::SaveFailed,
                        AccountEventKind::Saving => pending.phase = Phase::Saving,
                        AccountEventKind::Completed(_) | AccountEventKind::Cancelled => {
                            self.pending = None
                        }
                        _ => {}
                    }
                    return Ok(Some(event));
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(None),
                Err(mpsc::TryRecvError::Disconnected) if self.stopping => return Ok(None),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let failure = BridgeFailure("认证结果通道已关闭".into());
                    self.failure = Some(failure.clone());
                    return Err(failure);
                }
            }
        }
    }
    fn protocol_failure(&mut self) -> Result<Option<BridgeEvent>, BridgeFailure> {
        let failure = BridgeFailure("账号结果不属于当前操作".into());
        self.failure = Some(failure.clone());
        Err(failure)
    }
    pub fn sign_out(&mut self) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("认证运行已停止".into()));
        }
        if let Some(pending) = &mut self.sign_out {
            if !pending.failed {
                return Err(BridgeFailure("退出登录正在进行".into()));
            }
            self.runtime
                .sign_out(pending.id)
                .map_err(|failure| BridgeFailure(failure.to_string()))?;
            pending.failed = false;
            return Ok(pending.id);
        }
        let id = self
            .operations
            .allocate()
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.runtime
            .sign_out(id)
            .map_err(|failure| BridgeFailure(failure.to_string()))?;
        self.sign_out = Some(PendingSignOut { id, failed: false });
        self.take_progress()?;
        self.deferred = None;
        Ok(id)
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
