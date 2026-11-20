use crate::active::{ActiveState, ActiveTurn};
use crate::control::SteerView;
use crate::queue::QueuedInput;
use agentelier_llm_model::{OutputMessage, ProtocolHistory, ProtocolItemSummary};
use agentelier_prompts::{InstructionMessage, InstructionSnapshot};
use agentelier_tools::{ToolCatalog, ToolRegistration, ToolStatus};
use std::sync::Arc;
use std::{error::Error, fmt};

pub use uuid::Uuid as SessionId;
pub const MAX_TURN_REQUESTS: u32 = 32;
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TurnId(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompactionContinuation {
    pub request: u32,
    pub input: TurnId,
    pub input_retained: bool,
}

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
    Open {
        session: SessionId,
    },
    Delete {
        session: SessionId,
    },
    Create(SessionSettings),
    Reset {
        session: SessionId,
    },
    Send {
        session: SessionId,
        input: String,
    },
    Retry {
        session: SessionId,
        turn: TurnId,
    },
    ContinueWithoutCompaction {
        session: SessionId,
        turn: TurnId,
        failed_request: u32,
    },
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
    #[serde(default)]
    pub source: Option<String>,
    pub arguments: String,
    pub arguments_truncated: bool,
    pub status: ToolStatus,
    pub output: Option<String>,
    pub retained: bool,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_continuation: Option<CompactionContinuation>,
    #[serde(default)]
    pub selected_skills: Vec<agentelier_skills::SelectedSkill>,
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
    pub(crate) process_cleanup_unconfirmed: bool,
    pub tool_catalog: Vec<(String, String)>,
    pub queue: Vec<QueuedInput>,
    pub queue_paused: bool,
    pub task_policy: Box<crate::TaskPolicy>,
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
    Tasks(crate::SessionTasks),
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

pub(crate) struct ChatSession {
    pub(crate) processes: crate::ProcessLedger,
    pub(crate) memory: Option<Arc<agentelier_llm_model::MemorySnapshot>>,
    pub(crate) view: SessionView,
    pub(crate) history: Option<ProtocolHistory>,
    pub(crate) tools: Arc<ToolCatalog>,
    pub(crate) pending_results: std::collections::BTreeMap<String, crate::active::ToolResultEntry>,
    pub(crate) active: Option<ActiveState>,
    pub(crate) tasks: crate::tasks::TaskLedger,
}

pub struct PreparedSession {
    session: ChatSession,
    pub initial: crate::InitialSession,
}

#[derive(Default)]
pub struct Conversation {
    pub(crate) session: Option<ChatSession>,
    pub(crate) next_turn: u64,
    pub(crate) extensions: Arc<[ToolRegistration]>,
    pub(crate) queue_pause_revision: u64,
}

impl Conversation {
    pub fn restored_processes(&self) -> crate::ProcessLedger {
        self.session
            .as_ref()
            .map_or_else(crate::ProcessLedger::default, |session| {
                session.processes.clone()
            })
    }
    pub fn with_extensions(extensions: Vec<ToolRegistration>) -> Self {
        Self {
            extensions: extensions.into(),
            ..Self::default()
        }
    }

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

    pub fn tasks(&self) -> Option<crate::SessionTasks> {
        self.session
            .as_ref()
            .map(|session| session.tasks.view(session.view.id))
    }

    pub fn settle_tasks(&mut self, sequence: u64) -> bool {
        self.session.as_mut().is_some_and(|session| {
            session
                .view
                .turn
                .as_ref()
                .is_some_and(|turn| session.tasks.settle_turn(turn.id, sequence))
        })
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

    pub fn check_replacement(&self) -> Result<(), ConversationFailure> {
        if let Some(current) = &self.session {
            if current.active.is_some() {
                return Err(ConversationFailure::new("当前会话仍有活动任务"));
            }
            if current.view.process_cleanup_unconfirmed {
                return Err(ConversationFailure::new("当前会话的进程回收仍未确认"));
            }
            if current
                .pending_results
                .values()
                .any(|result| result.saved.is_none())
            {
                return Err(ConversationFailure::new("当前会话仍有工具结果未确认保存"));
            }
        }
        Ok(())
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
        discovered: &[ToolRegistration],
        memory: Option<agentelier_llm_model::MemorySnapshot>,
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
            ToolCatalog::with_extensions(
                &settings.working_directory,
                settings.image_input,
                if settings.slug == "gpt-6.2-sol" {
                    agentelier_tools::ToolContract::AdditionalTools
                } else {
                    agentelier_tools::ToolContract::Flat
                },
                self.extensions.iter().chain(discovered),
            )
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?,
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
            process_cleanup_unconfirmed: false,
            queue: Vec::new(),
            queue_paused: false,
            task_policy: Default::default(),
            tool_catalog: tools
                .definitions()
                .map(|tool| (tool.name.clone(), tool.catalog_description()))
                .collect(),
        };
        if let Some(snapshot) = &memory {
            session.context.task_material_tokens = snapshot.estimated_tokens();
        }
        session.context.limits = session.settings.context_limits.as_ref().clone();
        session.context.refresh(
            &ProtocolHistory::default(),
            &session.settings.instructions,
            tools.definitions(),
        )?;
        let initial = crate::InitialSession::from_view(&session, &tools, memory.clone())?;
        Ok(PreparedSession {
            initial,
            session: ChatSession {
                processes: Default::default(),
                memory: memory.map(Arc::new),
                view: session,
                history: Some(ProtocolHistory::default()),
                tools,
                pending_results: Default::default(),
                active: None,
                tasks: Default::default(),
            },
        })
    }

    pub fn install(&mut self, prepared: PreparedSession) -> SessionView {
        let view = prepared.session.view.clone();
        self.session = Some(prepared.session);
        self.next_turn = 0;
        view
    }

    pub fn prepare_reset(
        &self,
        id: SessionId,
        tools: &[ToolRegistration],
        memory: Option<agentelier_llm_model::MemorySnapshot>,
    ) -> Result<PreparedSession, ConversationFailure> {
        let settings = self
            .session
            .as_ref()
            .filter(|current| current.view.id == id)
            .ok_or(ConversationFailure::new("要重置的会话已变化"))?
            .view
            .settings
            .clone();
        self.prepare(settings, tools, memory)
    }

    pub fn begin(
        &mut self,
        operation_id: u64,
        session_id: SessionId,
        input: String,
    ) -> Result<SessionView, ConversationFailure> {
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
        let history = session
            .history
            .as_ref()
            .ok_or(ConversationFailure::new("会话缺少历史"))?;
        session.check_pending_input(history, &input)?;
        let request_items = history.with_user(&input).summary();
        session.view.notice = None;
        session.view.task_policy.background_paused = false;
        if let Some(previous) = session.view.turn.take() {
            session.view.previous_turns.push(*previous);
        }
        session.view.turn = Some(Box::new(TurnView {
            compaction_continuation: None,
            selected_skills: Vec::new(),
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
        session.active = Some(ActiveState {
            memory: session.memory.clone(),
            source_sequence: None,
            responses: Default::default(),
            history_confirmed: true,
            input_retained: false,
            accepting_steers: true,
            completion: None,
            continuation: None,
        });
        Ok(session.view.clone())
    }

    pub fn start(
        &mut self,
        operation: u64,
        request: ConversationRequest,
    ) -> Result<SessionView, ConversationFailure> {
        match request {
            ConversationRequest::Send { session, input } => self.begin(operation, session, input),
            ConversationRequest::ContinueWithoutCompaction {
                session,
                turn,
                failed_request,
            } => self.continue_without_compaction(operation, session, turn, failed_request),
            ConversationRequest::Retry { session, turn } => {
                let previous = self
                    .session
                    .as_ref()
                    .filter(|current| current.view.id == session)
                    .and_then(|current| current.view.turn.as_ref())
                    .filter(|previous| {
                        previous.id == turn
                            && previous.compaction_continuation.is_none()
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
                self.begin(operation, session, input)?;
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
                Ok(current.view.clone())
            }
            ConversationRequest::Create(_)
            | ConversationRequest::Reset { .. }
            | ConversationRequest::Delete { .. }
            | ConversationRequest::Open { .. } => {
                Err(ConversationFailure::new("创建会话不是生成操作"))
            }
        }
    }

    fn continue_without_compaction(
        &mut self,
        operation: u64,
        session: SessionId,
        turn: TurnId,
        failed_request: u32,
    ) -> Result<SessionView, ConversationFailure> {
        let current = self
            .session
            .as_mut()
            .filter(|current| current.view.id == session && !current.view.execution_blocked)
            .ok_or_else(|| ConversationFailure::new("原会话已变化或暂停执行"))?;
        let turn = current
            .view
            .turn
            .as_mut()
            .filter(|source| {
                source.id == turn
                    && source.request_index == failed_request
                    && matches!(source.status, TurnStatus::Failed(_))
            })
            .ok_or_else(|| ConversationFailure::new("失败请求已变化，不能使用原压缩继续操作"))?;
        let source = turn
            .compaction_continuation
            .filter(|source| {
                source.request == failed_request
                    && source.input == turn.input_id
                    && failed_request < MAX_TURN_REQUESTS
            })
            .ok_or_else(|| ConversationFailure::new("压缩继续操作已用完或超出本轮请求预算"))?;
        let history = current
            .history
            .as_ref()
            .ok_or_else(|| ConversationFailure::new("会话缺少已确认历史"))?;
        if history.has_open_calls() {
            return Err(ConversationFailure::new("等待当前批次结果确认后才能继续"));
        }
        let history = if source.input_retained {
            history.clone()
        } else {
            history.with_user(&turn.input)
        };
        let mut context = (*current.view.context).clone();
        context.refresh(
            &history,
            &current.view.settings.instructions,
            current.tools.definitions(),
        )?;
        let fixed = turn
            .requests
            .last()
            .and_then(|request| request.projection.fixed_bytes)
            .unwrap_or_default();
        crate::result_budget::ResultHeadroom::default().ensure(
            &context,
            fixed.saturating_add(
                history
                    .input_wire_bytes()
                    .map_err(crate::result_budget::model_error)?,
            ),
        )?;
        turn.status = TurnStatus::Running;
        turn.operation_id = operation;
        turn.compaction_continuation = None;
        current.view.notice = None;
        current.active = Some(ActiveState {
            memory: current.memory.clone(),
            source_sequence: None,
            responses: Default::default(),
            history_confirmed: true,
            input_retained: source.input_retained,
            accepting_steers: true,
            completion: None,
            continuation: Some(source),
        });
        Ok(current.view.clone())
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

    pub fn active_turn(
        &mut self,
        operation_id: u64,
    ) -> Result<ActiveTurn<'_>, ConversationFailure> {
        self.turn(operation_id)?;
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("任务缺少会话"))?;
        Ok(ActiveTurn {
            state: session
                .active
                .as_mut()
                .ok_or_else(|| ConversationFailure::new("当前任务缺少活动状态"))?,
            view: &mut session.view,
            history: session
                .history
                .as_mut()
                .ok_or_else(|| ConversationFailure::new("当前任务缺少会话历史"))?,
            tools: &session.tools,
            results: &mut session.pending_results,
            tasks: &mut session.tasks,
        })
    }

    pub fn finish(&mut self, operation_id: u64) -> Result<SessionView, ConversationFailure> {
        let active = self.active_turn(operation_id)?;
        let completion = active
            .state
            .completion
            .take()
            .ok_or_else(|| ConversationFailure::new("任务回收时缺少终态"))?;
        let turn = self.turn(operation_id)?;
        turn.status = completion.clone();
        let turn_id = turn.id;
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("任务缺少会话"))?;
        session.active = None;
        session.tasks.close_execution(turn_id);
        if !matches!(completion, TurnStatus::Completed) {
            session.view.queue_paused = true;
        }
        Ok(session.view.clone())
    }

    pub fn interrupt(&mut self, operation_id: u64) -> Result<SessionView, ConversationFailure> {
        let turn = self.turn(operation_id)?;
        turn.status = TurnStatus::Failed("任务异常结束，执行记录未确认，请重新加载存档".into());
        let turn_id = turn.id;
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("任务缺少会话"))?;
        session.view.execution_blocked = true;
        session.view.queue_paused = true;
        session.history = None;
        session.active = None;
        session.tasks.close_execution(turn_id);
        Ok(session.view.clone())
    }
}
