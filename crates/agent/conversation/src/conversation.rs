use crate::active::{ActiveState, ActiveTurn};
use crate::control::SteerView;
use crate::queue::QueuedInput;
use crate::{
    AgentId, AgentRegistry, AgentRegistryError, AgentResidency, AgentRuntimeId, AgentState,
};
use agentelier_llm_model::{OutputMessage, ProtocolHistory, ProtocolItemSummary};
use agentelier_prompts::{InstructionMessage, InstructionSnapshot};
use agentelier_tools::{ToolCatalog, ToolRegistration, ToolStatus};
use std::sync::Arc;
use std::{error::Error, fmt};

pub use uuid::Uuid as SessionId;
pub const MAX_TURN_REQUESTS: u32 = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TurnId(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompactionContinuation {
    pub request: u32,
    pub input: TurnId,
    pub input_retained: bool,
}

#[derive(Clone)]
pub struct SessionSettings {
    pub codex: Option<agentelier_llm_model::CodexIdentity>,
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub image_input: Option<bool>,
    pub context_limits: Box<agentelier_llm_model::ContextLimits>,
    pub instructions: Arc<InstructionSnapshot>,
    pub working_directory: std::path::PathBuf,
}

impl SessionSettings {
    pub fn validate_source(
        &self,
        codex: &agentelier_llm_model::CodexIdentity,
    ) -> Result<(), ConversationFailure> {
        let bound = self.codex.as_ref().ok_or_else(|| {
            ConversationFailure::new(
                "此会话的 SIWC 授权已下线。历史可查看，请新建 Codex 会话继续聊天",
            )
        })?;
        if bound != codex {
            return Err(ConversationFailure::new(
                "本会话绑定的账号或工作区与当前 Codex 凭据不一致，请新建会话",
            ));
        }
        Ok(())
    }
}
#[derive(Clone)]
pub enum SessionLocation {
    Standalone(std::path::PathBuf),
    InProject {
        project: crate::ProjectId,
        directory_override: Option<std::path::PathBuf>,
    },
}

#[derive(Clone)]
pub struct SessionCreate {
    pub slug: String,
    pub effort: String,
    pub instructions: Arc<InstructionSnapshot>,
    pub location: SessionLocation,
}

impl SessionCreate {
    pub fn resolve(
        self,
        source: agentelier_llm_model::CodexIdentity,
        catalog: &[agentelier_llm_model::ModelInfo],
        working_directory: std::path::PathBuf,
    ) -> Result<SessionSettings, ConversationFailure> {
        let model = agentelier_llm_model::select_session_model(catalog, &self.slug, &self.effort)
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        Ok(SessionSettings {
            codex: Some(source),
            slug: self.slug,
            display_name: model.display_name.clone(),
            effort: Some(self.effort),
            image_input: model.image_input,
            context_limits: Box::new(model.context_limits.clone()),
            instructions: self.instructions,
            working_directory,
        })
    }
}

pub enum ConversationRequest {
    ProcessResults {
        session: SessionId,
    },
    Open {
        session: SessionId,
    },
    Delete {
        session: SessionId,
    },
    Create(SessionCreate),
    Reset {
        session: SessionId,
    },
    SetWorkingDirectory {
        session: SessionId,
        directory: std::path::PathBuf,
    },
    Send {
        session: SessionId,
        input: String,
        audio: Option<crate::AudioId>,
        transcript: Option<crate::TranscriptId>,
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collaboration: bool,
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
    #[serde(skip)]
    pub image: Option<ToolImage>,
    pub retained: bool,
}

#[derive(Clone)]
pub enum ToolImage {
    Attachment(crate::StoredImage),
    Generated(agentelier_tools::ImageDescriptor),
}

impl ToolImage {
    pub fn descriptor(&self) -> &agentelier_tools::ImageDescriptor {
        match self {
            Self::Attachment(stored) => &stored.image,
            Self::Generated(image) => image,
        }
    }
}

pub(crate) fn is_collaboration_tool(tool: &agentelier_tools::ToolDefinition) -> bool {
    matches!(&tool.source, agentelier_tools::ToolSource::Extension { provider, .. }
        if provider == "agentelier.collaboration")
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnView {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub result_continuation: bool,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<crate::AudioAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<agentelier_llm_model::TranscriptReference>,
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
    pub settings: Box<SessionSettings>,
    pub turn: Option<Box<TurnView>>,
    pub previous_turns: Vec<TurnView>,
    pub audio_draft: Option<crate::AudioAttachment>,
    pub transcriptions: Arc<Vec<crate::AudioTranscription>>,
    pub committed_items: Vec<ProtocolItemSummary>,
    pub execution_blocked: bool,
    pub(crate) process_cleanup_unconfirmed: bool,
    pub tool_catalog: Vec<(String, String)>,
    pub queue: Vec<QueuedInput>,
    pub queue_paused: bool,
    pub task_policy: Box<crate::TaskPolicy>,
}
pub enum ConversationEvent {
    IdleUpdated(SessionView),
    AudioDraft {
        session: SessionId,
        result: Result<Option<crate::AudioAttachment>, String>,
    },
    AudioTranscription {
        session: SessionId,
        result: Result<crate::AudioTranscription, String>,
    },
    Processes(crate::SessionProcesses),
    TaskAction(crate::TaskActionReceipt),
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
    Projects(Result<crate::ProjectCatalog, String>),
    ProjectSaved(Result<(), String>),
    Opened(SessionView),
    ArchiveFailed(String),
    Tasks(crate::SessionTasks),
    GlobalTasks(crate::GlobalTasks),
    TaskNotification(crate::TaskNotification),
    Created(SessionView),
    WorkingDirectorySaved(SessionView),
    SessionModelSaved {
        session: SessionId,
        result: Result<SessionView, String>,
    },
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
    pub(crate) idle: Option<ActiveState>,
    pub(crate) processes: crate::ProcessLedger,
    pub(crate) memory: Option<Arc<agentelier_llm_model::MemorySnapshot>>,
    pub(crate) view: SessionView,
    pub(crate) history: Option<ProtocolHistory>,
    pub(crate) tools: Arc<ToolCatalog>,
    pub(crate) pending_results: std::collections::BTreeMap<String, crate::active::ToolResultEntry>,
    pub(crate) active: Option<ActiveState>,
    pub(crate) tasks: crate::tasks::TaskLedger,
    pub(crate) child: Option<crate::ChildMaterials>,
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
    agents: AgentRegistry,
    pub(crate) mailboxes: crate::Mailboxes,
}

impl Conversation {
    pub fn validate_model_source(
        &self,
        codex: &agentelier_llm_model::CodexIdentity,
    ) -> Result<(), ConversationFailure> {
        self.session.as_ref().map_or(Ok(()), |session| {
            session.view.settings.validate_source(codex)
        })
    }

    pub fn restored_processes(&self) -> crate::ProcessLedger {
        self.session
            .as_ref()
            .map_or_else(crate::ProcessLedger::default, |session| {
                session.processes.clone()
            })
    }

    pub fn retain_closed_processes(
        &mut self,
        id: SessionId,
        mut processes: crate::ProcessLedger,
    ) -> Result<crate::SessionProcesses, ConversationFailure> {
        let session = self
            .session
            .as_mut()
            .filter(|session| session.view.id == id)
            .ok_or_else(|| ConversationFailure::new("账号切换时的进程记录属于另一会话"))?;
        processes.historical();
        let view = processes.projection(id);
        session.processes = processes;
        Ok(view)
    }

    pub fn matches_task_execution(
        &self,
        session: SessionId,
        task: crate::TaskId,
        instance: SessionId,
    ) -> bool {
        self.session
            .as_ref()
            .filter(|current| current.view.id == session)
            .is_some_and(|current| current.tasks.matches_execution(task, instance))
    }
    pub fn with_extensions(extensions: Vec<ToolRegistration>) -> Self {
        Self {
            extensions: extensions.into(),
            ..Self::default()
        }
    }

    pub fn remove(&mut self, id: SessionId) {
        self.agents.retire_session(id);
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.view.id == id)
        {
            self.session = None;
            self.next_turn = 0;
            self.mailboxes = crate::Mailboxes::default();
        }
    }
    pub fn current(&self) -> Option<SessionView> {
        self.session.as_ref().map(|session| session.view.clone())
    }

    pub fn session_id(&self) -> Option<SessionId> {
        self.session.as_ref().map(|session| session.view.id)
    }

    pub fn empty_session_owner(&self) -> Self {
        Self {
            extensions: self.extensions.clone(),
            ..Self::default()
        }
    }

    pub fn tool_image(&self, session: SessionId, call_id: &str) -> Option<ToolImage> {
        let view = &self
            .session
            .as_ref()
            .filter(|chat| chat.view.id == session)?
            .view;
        view.turn
            .iter()
            .map(Box::as_ref)
            .chain(&view.previous_turns)
            .flat_map(|turn| &turn.tools)
            .find(|tool| tool.retained && tool.call_id == call_id)?
            .image
            .clone()
    }

    pub fn tasks(&self) -> Option<crate::SessionTasks> {
        self.session.as_ref().map(|session| {
            session.tasks.view(
                session.view.id,
                &session.view.task_policy,
                session.view.queue_paused,
                Some(&session.tools),
            )
        })
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

    pub fn prepare_working_directory(
        &self,
        session: SessionId,
        directory: &std::path::Path,
    ) -> Result<std::path::PathBuf, ConversationFailure> {
        let current = self
            .current()
            .filter(|current| current.id == session)
            .ok_or_else(|| ConversationFailure::new("当前会话已变化"))?;
        if current.execution_blocked {
            return Err(ConversationFailure::new("当前会话处于只读状态"));
        }
        let directory = agentelier_tools::WorkingDirectory::validate(directory)
            .map_err(|failure| ConversationFailure::new(format!("本地工作目录无效：{failure}")))?;
        Ok(directory.path().to_path_buf())
    }

    pub fn install_working_directory(
        &mut self,
        session: SessionId,
        directory: std::path::PathBuf,
    ) -> Result<SessionView, ConversationFailure> {
        let current = self
            .session
            .as_mut()
            .filter(|current| current.view.id == session)
            .ok_or_else(|| ConversationFailure::new("目录保存后无法找到原会话"))?;
        if current.view.settings.working_directory != directory {
            current.view.settings.working_directory = directory;
            *current.view.project_context = crate::ProjectContextState::NotCollected;
        }
        Ok(current.view.clone())
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
        if settings.codex.is_none() {
            return Err(ConversationFailure::new(
                "新会话需要绑定有效的 Codex 账号和工作区",
            ));
        }
        let id = SessionId::new_v4();
        self.agents
            .ensure_available(id, AgentId::for_root(id))
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
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
                settings.image_input,
                agentelier_llm_model::codex_tool_contract(&settings.slug),
                settings.codex.is_some(),
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
            settings: Box::new(settings),
            turn: None,
            previous_turns: Vec::new(),
            audio_draft: None,
            transcriptions: Default::default(),
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
                idle: None,
                processes: Default::default(),
                memory: memory.map(Arc::new),
                view: session,
                history: Some(ProtocolHistory::default()),
                tools,
                pending_results: Default::default(),
                active: None,
                tasks: Default::default(),
                child: None,
            },
        })
    }

    pub fn install(
        &mut self,
        prepared: PreparedSession,
    ) -> Result<(SessionView, AgentRuntimeId), AgentRegistryError> {
        let view = prepared.session.view.clone();
        let instance = self.activate_root(&view)?;
        self.session = Some(prepared.session);
        self.next_turn = 0;
        self.mailboxes = crate::Mailboxes::default();
        Ok((view, instance))
    }

    pub(crate) fn activate_root(
        &mut self,
        view: &SessionView,
    ) -> Result<AgentRuntimeId, AgentRegistryError> {
        let root = AgentId::for_root(view.id);
        self.agents.register_root(view.id, root)?;
        self.sync_root_state(view)?;
        if let Some(current) = &self.session
            && let Some(instance) = self.root_instance(current.view.id)
        {
            let session = current.view.id;
            self.agents
                .detach(session, AgentId::for_root(session), instance)?;
        }
        self.agents.attach(view.id, root)
    }

    pub fn from_child(
        thread: SessionId,
        prepared: crate::PreparedChild,
    ) -> Result<Self, ConversationFailure> {
        let crate::PreparedChild {
            config,
            history,
            tools,
        } = prepared;
        let settings = config.settings;
        let publications: Vec<_> = config.projects.into_iter().map(Arc::new).collect();
        let project = publications
            .last()
            .ok_or_else(|| ConversationFailure::new("子 Agent 缺少冻结的项目来源"))?;
        let window = project.window;
        let child = crate::ChildMaterials {
            project: Arc::new(project.snapshot.clone()),
            skills: Arc::new(config.skills),
        };
        let mut view = SessionView {
            project_publications: Arc::new(publications),
            project_context: Box::new(crate::ProjectContextState::Loaded(child.project.clone())),
            context: Box::new(crate::ContextState {
                window,
                limits: settings.context_limits.as_ref().clone(),
                ..Default::default()
            }),
            notice: None,
            id: thread,
            settings: Box::new(SessionSettings {
                codex: settings.codex,
                slug: settings.slug,
                display_name: settings.display_name,
                effort: settings.effort,
                image_input: settings.image_input,
                context_limits: settings.context_limits,
                instructions: Arc::new(settings.instructions),
                working_directory: settings.working_directory,
            }),
            turn: None,
            previous_turns: Vec::new(),
            audio_draft: None,
            transcriptions: Default::default(),
            committed_items: history.summary(),
            execution_blocked: false,
            process_cleanup_unconfirmed: false,
            tool_catalog: tools
                .definitions()
                .map(|tool| (tool.name.clone(), tool.catalog_description()))
                .collect(),
            queue: Vec::new(),
            queue_paused: false,
            task_policy: Default::default(),
        };
        view.context
            .refresh(&history, &view.settings.instructions, tools.definitions())?;
        Ok(Self {
            session: Some(ChatSession {
                idle: None,
                processes: Default::default(),
                memory: None,
                view,
                history: Some(history),
                tools,
                pending_results: Default::default(),
                active: None,
                tasks: Default::default(),
                child: Some(child),
            }),
            ..Self::default()
        })
    }

    pub(crate) fn activate_restored_root(
        &mut self,
        view: &SessionView,
        agents: Vec<(crate::AgentIdentity, AgentState)>,
        stops: Vec<crate::AgentStop>,
    ) -> Result<AgentRuntimeId, AgentRegistryError> {
        let backup = self.agents.clone();
        let result = (|| {
            let instance = self.activate_root(view)?;
            for (identity, state) in agents {
                let id = identity.id;
                self.agents.restore_child(identity)?;
                let state = match state {
                    AgentState::Starting | AgentState::Running => AgentState::Unknown,
                    state => state,
                };
                self.agents.set_state(view.id, id, state)?;
            }
            self.agents.clear_reservations(view.id);
            for stop in stops {
                self.agents.remember_stop(view.id, stop);
            }
            Ok(instance)
        })();
        if result.is_err() {
            self.agents = backup;
        }
        result
    }

    pub fn sync_root_state(&mut self, view: &SessionView) -> Result<(), AgentRegistryError> {
        let state = match view.turn.as_ref().map(|turn| &turn.status) {
            None => AgentState::Waiting,
            Some(TurnStatus::Running) => AgentState::Running,
            Some(TurnStatus::Completed) => AgentState::Completed,
            Some(TurnStatus::Cancelled) => AgentState::Interrupted,
            Some(TurnStatus::Limited) => AgentState::Failed("模型达到本轮执行预算".into()),
            Some(TurnStatus::Failed(reason)) => AgentState::Failed(reason.clone()),
        };
        self.agents
            .set_state(view.id, AgentId::for_root(view.id), state)
    }

    pub fn agent_registry(&self) -> &AgentRegistry {
        &self.agents
    }

    pub fn agent_registry_mut(&mut self) -> &mut AgentRegistry {
        &mut self.agents
    }

    pub fn mailboxes(&self) -> &crate::Mailboxes {
        &self.mailboxes
    }

    pub fn mailboxes_mut(&mut self) -> &mut crate::Mailboxes {
        &mut self.mailboxes
    }

    pub fn root_instance(&self, session: SessionId) -> Option<AgentRuntimeId> {
        self.session
            .as_ref()
            .filter(|current| current.view.id == session)?;
        match self
            .agents
            .get(session, AgentId::for_root(session))
            .ok()?
            .residency
        {
            AgentResidency::Resident(instance) => Some(instance),
            AgentResidency::Unloaded => None,
        }
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
            .as_ref()
            .clone();
        self.prepare(settings, tools, memory)
    }

    pub(crate) fn begin_input(
        &mut self,
        operation_id: u64,
        session_id: SessionId,
        input: Option<String>,
        audio: Option<crate::AudioId>,
        transcript: Option<agentelier_llm_model::TranscriptReference>,
    ) -> Result<SessionView, ConversationFailure> {
        if input.as_ref().is_some_and(|input| input.trim().is_empty()) {
            return Err(ConversationFailure::new("不能发送空白消息"));
        }
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("尚未创建聊天会话"))?;
        if session.view.id != session_id {
            return Err(ConversationFailure::new("聊天会话身份已变化"));
        }
        if audio.is_some() && session.view.audio_draft.as_ref().map(|item| item.id) != audio {
            return Err(ConversationFailure::new(
                "草稿音频附件已变化，请核对后重新发送",
            ));
        }
        let evidence = transcript
            .as_ref()
            .map(|reference| session.view.transcript_evidence(reference))
            .transpose()?;
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
        let result_continuation = input.is_none();
        if result_continuation && !session.tasks.has_ready_result() {
            return Err(ConversationFailure::new("当前没有已保存、待处理的工具结果"));
        }
        let request_items = if let Some(input) = &input {
            session.check_pending_input(history, input, evidence.as_ref())?;
            history.with_user(input).summary()
        } else {
            history.summary()
        };
        session.view.notice = None;
        if !result_continuation {
            session.view.task_policy.background_paused = false;
        }
        if let Some(previous) = session.view.turn.take() {
            session.view.previous_turns.push(*previous);
        }
        session.view.turn = Some(Box::new(TurnView {
            result_continuation,
            compaction_continuation: None,
            selected_skills: Vec::new(),
            skills: None,
            requests: Vec::new(),
            id: TurnId(id),
            input_id: TurnId(id),
            is_retry: false,
            operation_id,
            input: input.unwrap_or_default(),
            audio: audio.and_then(|_| session.view.audio_draft.take()),
            transcript,
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
            responses: session
                .idle
                .take()
                .map_or_else(Default::default, |idle| idle.responses),
            history_confirmed: true,
            input_retained: result_continuation,
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
            ConversationRequest::ProcessResults { session } => {
                self.begin_input(operation, session, None, None, None)
            }
            ConversationRequest::Send {
                session,
                input,
                audio,
                transcript,
            } => {
                let source = transcript
                    .map(|id| {
                        self.session
                            .as_ref()
                            .filter(|chat| chat.view.id == session)
                            .ok_or_else(|| ConversationFailure::new("聊天会话已变化"))?
                            .view
                            .transcript_reference(id)
                    })
                    .transpose()?;
                self.begin_input(operation, session, Some(input), audio, source)
            }
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
                let audio = previous.audio.clone();
                let transcript = previous.transcript.clone();
                self.begin_input(operation, session, Some(input), None, transcript)?;
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
                turn.audio = audio;
                turn.is_retry = true;
                Ok(current.view.clone())
            }
            ConversationRequest::Create(_)
            | ConversationRequest::Reset { .. }
            | ConversationRequest::SetWorkingDirectory { .. }
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
            responses: current
                .idle
                .take()
                .map_or_else(Default::default, |idle| idle.responses),
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
        self.tool_turn()
    }

    pub fn tool_turn(&mut self) -> Result<ActiveTurn<'_>, ConversationFailure> {
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("任务缺少会话"))?;
        Ok(ActiveTurn {
            state: session
                .active
                .as_mut()
                .or(session.idle.as_mut())
                .ok_or_else(|| ConversationFailure::new("当前任务缺少活动状态"))?,
            view: &mut session.view,
            history: session
                .history
                .as_mut()
                .ok_or_else(|| ConversationFailure::new("当前任务缺少会话历史"))?,
            tools: &session.tools,
            child: session.child.as_ref(),
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
        session.idle = session.active.take();
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
        session.idle = session.active.take();
        session.tasks.close_execution(turn_id);
        Ok(session.view.clone())
    }
}
