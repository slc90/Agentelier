use crate::active::ActiveTurn;
use crate::control::SteerView;
use crate::queue::QueuedInput;
use agentelier_llm_model::{OutputMessage, ProtocolHistory, ProtocolItemSummary};
use agentelier_prompts::{InstructionMessage, InstructionSnapshot};
use agentelier_tools::{ToolCatalog, ToolStatus};
use std::sync::Arc;
use std::{error::Error, fmt};

pub use uuid::Uuid as SessionId;
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TurnId(pub u64);

#[derive(Clone)]
pub struct SessionSettings {
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub image_input: Option<bool>,
    pub context_limits: Box<agentelier_llm_model::ContextLimits>,
    pub instructions: Arc<InstructionSnapshot>,
    pub working_directory: std::path::PathBuf,
}
pub enum ConversationRequest {
    Open { session: SessionId },
    Delete { session: SessionId },
    Create(SessionSettings),
    Reset { session: SessionId },
    Send { session: SessionId, input: String },
    Retry { session: SessionId, turn: TurnId },
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub enum TurnStatus {
    Running,
    Completed,
    Cancelled,
    Failed(String),
    Limited,
}
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum TaskPhase {
    LoadingProjectContext,
    CompactingContext,
    RequestingModel,
    ExecutingTool,
    ReapingProcesses,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolView {
    pub request_index: u32,
    pub id: String,
    pub output_index: usize,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub arguments_truncated: bool,
    pub status: ToolStatus,
    pub output: Option<String>,
    pub retained: bool,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnView {
    #[serde(default)]
    pub skills: Option<Box<agentelier_skills::Catalog>>,
    #[serde(default)]
    pub requests: Vec<crate::PreparedRequestFact>,
    pub id: TurnId,
    pub operation_id: u64,
    pub input_id: TurnId,
    pub is_retry: bool,
    pub input: String,
    pub messages: Vec<OutputMessage>,
    pub status: TurnStatus,
    pub request_items: Vec<ProtocolItemSummary>,
    pub request_index: u32,
    pub committed_steps: u32,
    pub tools: Vec<ToolView>,
    pub phase: TaskPhase,
    pub steers: Vec<SteerView>,
    pub request_instructions: Vec<InstructionMessage>,
    pub process_cleanup: Vec<agentelier_tools::ProcessCleanup>,
}
#[derive(Clone)]
pub struct SessionView {
    pub project_publications: Arc<Vec<Arc<agentelier_llm_model::ProjectPublication>>>,
    pub project_context: Box<crate::ProjectContextState>,
    pub context: Box<crate::ContextState>,
    pub notice: Option<String>,
    pub id: SessionId,
    pub settings: SessionSettings,
    pub turn: Option<Box<TurnView>>,
    pub previous_turns: Vec<TurnView>,
    pub committed_items: Vec<ProtocolItemSummary>,
    pub execution_blocked: bool,
    pub tool_catalog: Vec<(String, String)>,
    pub queue: Vec<QueuedInput>,
    pub queue_paused: bool,
}
pub enum ConversationEvent {
    ArchiveCleanup(Vec<String>),
    Deleted {
        session: SessionId,
        cleanup_error: Option<String>,
        was_open: bool,
    },
    Renamed {
        session: SessionId,
        result: Result<crate::ArchiveEntry, String>,
    },
    Archives(Vec<crate::ArchiveEntry>),
    Opened(SessionView),
    ArchiveFailed(String),
    Created(SessionView),
    Accepted(SessionView),
    Updated(SessionView),
    Finished(SessionView),
    Rejected(String),
    InputReceipt(crate::control::InputReceipt),
    Queue(crate::queue::QueueEvent),
    Started(SessionView),
}

#[derive(Debug)]
pub struct ConversationFailure(String);
impl ConversationFailure {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
impl fmt::Display for ConversationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for ConversationFailure {}

#[derive(Clone)]
pub(crate) struct ChatSession {
    pub(crate) view: SessionView,
    pub(crate) history: Option<ProtocolHistory>,
    pub(crate) tools: Arc<ToolCatalog>,
    pub(crate) pending_results: std::collections::BTreeMap<String, agentelier_tools::ToolPayload>,
}

pub struct PreparedSession {
    session: ChatSession,
    pub initial: crate::InitialSession,
}

#[derive(Clone, Default)]
pub struct Conversation {
    pub(crate) session: Option<ChatSession>,
    pub(crate) next_turn: u64,
    pub(crate) queue_pause_revision: u64,
}

impl Conversation {
    pub fn remove(&mut self, id: SessionId) {
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.view.id == id)
        {
            self.session = None;
            self.next_turn = 0;
        }
    }
    pub fn current(&self) -> Option<SessionView> {
        self.session.as_ref().map(|session| session.view.clone())
    }

    pub fn archive(
        &self,
    ) -> Option<(
        crate::StoredState,
        Option<agentelier_llm_model::ProtocolArchive>,
    )> {
        self.session.as_ref().map(|session| {
            (
                crate::StoredState::from(&session.view),
                session.history.as_ref().map(ProtocolHistory::archive),
            )
        })
    }

    pub fn fail_storage(&mut self, message: String) -> Option<SessionView> {
        let session = self.session.as_mut()?;
        session.view.execution_blocked = true;
        session.view.queue_paused = true;
        session.view.notice = Some(message.clone());
        if let Some(turn) = &mut session.view.turn
            && !matches!(turn.status, TurnStatus::Running)
        {
            turn.status = TurnStatus::Failed(message);
        }
        Some(session.view.clone())
    }

    pub fn suspend_queue(&mut self) -> Result<Option<crate::RecordEvent>, ConversationFailure> {
        let Some(session) = &mut self.session else {
            return Ok(None);
        };
        self.queue_pause_revision = self
            .queue_pause_revision
            .checked_add(1)
            .ok_or_else(|| ConversationFailure::new("队列暂停修订已耗尽"))?;
        session.view.queue_paused = true;
        Ok(Some(crate::RecordEvent::QueueSuspended {
            state: crate::StoredState::from(&session.view),
        }))
    }

    pub fn prepare(
        &self,
        mut settings: SessionSettings,
    ) -> Result<PreparedSession, ConversationFailure> {
        if self
            .session
            .as_ref()
            .and_then(|current| current.view.turn.as_ref())
            .is_some_and(|turn| matches!(turn.status, TurnStatus::Running))
        {
            return Err(ConversationFailure::new("生成尚未结束，不能替换会话"));
        }
        if settings.slug.trim().is_empty()
            || settings
                .effort
                .as_ref()
                .is_some_and(|effort| effort.trim().is_empty())
        {
            return Err(ConversationFailure::new("会话设置缺少有效模型或推理选项"));
        }
        let id = SessionId::new_v4();
        settings
            .instructions
            .validate()
            .map_err(|_| ConversationFailure::new("会话指令快照无效"))?;
        settings.working_directory =
            agentelier_tools::WorkingDirectory::validate(&settings.working_directory)
                .map_err(|failure| {
                    ConversationFailure::new(format!("本地工作目录无效：{failure}"))
                })?
                .path()
                .to_path_buf();
        let tools = Arc::new(
            ToolCatalog::standard(&settings.working_directory, settings.image_input)
                .map_err(|_| ConversationFailure::new("工具目录配置无效"))?,
        );
        let mut session = SessionView {
            project_publications: Arc::default(),
            project_context: Default::default(),
            context: Box::default(),
            notice: None,
            id,
            settings,
            turn: None,
            previous_turns: Vec::new(),
            committed_items: Vec::new(),
            execution_blocked: false,
            queue: Vec::new(),
            queue_paused: false,
            tool_catalog: tools
                .definitions()
                .iter()
                .map(|tool| (tool.name.to_owned(), tool.description.to_owned()))
                .collect(),
        };
        session.context.limits = session.settings.context_limits.as_ref().clone();
        session.context.refresh(
            &ProtocolHistory::default(),
            &session.settings.instructions,
            &tools,
        )?;
        let initial = crate::InitialSession::from_view(
            &session,
            tools
                .definitions()
                .iter()
                .map(|tool| crate::StoredTool {
                    name: tool.name.into(),
                    description: tool.description.clone(),
                    parameters: tool.parameters.clone(),
                })
                .collect(),
        )?;
        Ok(PreparedSession {
            initial,
            session: ChatSession {
                view: session,
                history: Some(ProtocolHistory::default()),
                tools,
                pending_results: Default::default(),
            },
        })
    }

    pub fn install(&mut self, prepared: PreparedSession) -> SessionView {
        let view = prepared.session.view.clone();
        self.session = Some(prepared.session);
        self.next_turn = 0;
        view
    }

    pub fn prepare_reset(&self, id: SessionId) -> Result<PreparedSession, ConversationFailure> {
        let settings = self
            .session
            .as_ref()
            .filter(|current| current.view.id == id)
            .ok_or(ConversationFailure::new("要重置的会话已变化"))?
            .view
            .settings
            .clone();
        self.prepare(settings)
    }

    pub fn begin(
        &mut self,
        operation_id: u64,
        session_id: SessionId,
        input: String,
    ) -> Result<(SessionView, ActiveTurn), ConversationFailure> {
        if input.trim().is_empty() {
            return Err(ConversationFailure::new("不能发送空白消息"));
        }
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("尚未创建聊天会话"))?;
        if session.view.id != session_id {
            return Err(ConversationFailure::new("聊天会话身份已变化"));
        }
        if session
            .view
            .turn
            .as_ref()
            .is_some_and(|turn| matches!(turn.status, TurnStatus::Running))
        {
            return Err(ConversationFailure::new("上一轮尚未结束"));
        }
        let id = self
            .next_turn
            .checked_add(1)
            .ok_or(ConversationFailure::new("聊天轮次标识已耗尽"))?;
        if session.view.execution_blocked || session.history.is_none() {
            return Err(ConversationFailure::new(
                "会话执行记录不可用，请重置或新建会话",
            ));
        }
        let request_items = session
            .history
            .as_ref()
            .ok_or(ConversationFailure::new("会话缺少历史"))?
            .with_user(&input)
            .summary();
        session.view.notice = None;
        if let Some(previous) = session.view.turn.take() {
            session.view.previous_turns.push(*previous);
        }
        session.view.turn = Some(Box::new(TurnView {
            skills: None,
            requests: Vec::new(),
            id: TurnId(id),
            input_id: TurnId(id),
            is_retry: false,
            operation_id,
            input,
            messages: Vec::new(),
            status: TurnStatus::Running,
            request_items,
            request_index: 0,
            committed_steps: 0,
            tools: Vec::new(),
            phase: TaskPhase::LoadingProjectContext,
            steers: Vec::new(),
            request_instructions: session.view.settings.instructions.messages(),
            process_cleanup: Vec::new(),
        }));
        self.next_turn = id;
        let active = ActiveTurn {
            source_sequence: None,
            history_confirmed: true,
            view: session.view.clone(),
            tools: session.tools.clone(),
            input_retained: false,
            results: std::mem::take(&mut session.pending_results),
            history: session
                .history
                .take()
                .ok_or(ConversationFailure::new("会话缺少历史"))?,
        };
        Ok((session.view.clone(), active))
    }

    pub fn start(
        &mut self,
        operation: u64,
        request: ConversationRequest,
    ) -> Result<(SessionView, ActiveTurn), ConversationFailure> {
        match request {
            ConversationRequest::Send { session, input } => self.begin(operation, session, input),
            ConversationRequest::Retry { session, turn } => {
                let previous = self
                    .session
                    .as_ref()
                    .filter(|current| current.view.id == session)
                    .and_then(|current| current.view.turn.as_ref())
                    .filter(|previous| {
                        previous.id == turn
                            && previous.committed_steps == 0
                            && !previous.steers.iter().any(|input| {
                                matches!(input.state, crate::control::SteerState::Applied)
                            })
                            && matches!(
                                previous.status,
                                TurnStatus::Failed(_) | TurnStatus::Cancelled
                            )
                    })
                    .ok_or(ConversationFailure::new("只有最近失败或取消的输入可以重试"))?;
                let input = previous.input.clone();
                let input_id = previous.input_id;
                let (_, mut active) = self.begin(operation, session, input)?;
                let current = self
                    .session
                    .as_mut()
                    .ok_or(ConversationFailure::new("重试缺少会话"))?;
                let turn = current
                    .view
                    .turn
                    .as_mut()
                    .ok_or(ConversationFailure::new("重试缺少轮次"))?;
                turn.input_id = input_id;
                turn.is_retry = true;
                active.view = current.view.clone();
                Ok((current.view.clone(), active))
            }
            ConversationRequest::Create(_)
            | ConversationRequest::Reset { .. }
            | ConversationRequest::Delete { .. }
            | ConversationRequest::Open { .. } => {
                Err(ConversationFailure::new("创建会话不是生成操作"))
            }
        }
    }

    fn turn(&mut self, operation_id: u64) -> Result<&mut TurnView, ConversationFailure> {
        self.session
            .as_mut()
            .and_then(|session| session.view.turn.as_deref_mut())
            .filter(|turn| {
                turn.operation_id == operation_id && matches!(turn.status, TurnStatus::Running)
            })
            .ok_or(ConversationFailure::new("模型结果不属于当前运行轮次"))
    }

    pub fn update(
        &mut self,
        operation_id: u64,
        mut view: SessionView,
    ) -> Result<SessionView, ConversationFailure> {
        let turn = self.turn(operation_id)?;
        if view
            .turn
            .as_ref()
            .is_none_or(|incoming| incoming.id != turn.id || incoming.operation_id != operation_id)
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.view.id != view.id)
        {
            return Err(ConversationFailure::new("任务快照身份不匹配"));
        }
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("任务缺少会话"))?;
        view.queue = std::mem::take(&mut session.view.queue);
        view.execution_blocked |= session.view.execution_blocked;
        view.queue_paused = session.view.queue_paused;
        session.view = view;
        Ok(session.view.clone())
    }

    pub fn finish(
        &mut self,
        operation_id: u64,
        active: ActiveTurn,
    ) -> Result<SessionView, ConversationFailure> {
        let mut view = active.view;
        if view
            .turn
            .as_ref()
            .is_some_and(|turn| !matches!(turn.status, TurnStatus::Completed))
        {
            self.session
                .as_mut()
                .ok_or(ConversationFailure::new("任务缺少会话"))?
                .view
                .queue_paused = true;
        }
        view = self.update(operation_id, view)?;
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("任务缺少会话"))?;
        session.history = Some(active.history);
        session.pending_results = active.results;
        Ok(view)
    }

    pub fn interrupt(&mut self, operation_id: u64) -> Result<SessionView, ConversationFailure> {
        self.turn(operation_id)?.status =
            TurnStatus::Failed("任务异常结束，无法归还执行记录，请重置或新建会话".into());
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("任务缺少会话"))?;
        session.view.execution_blocked = true;
        session.view.queue_paused = true;
        Ok(session.view.clone())
    }
}
