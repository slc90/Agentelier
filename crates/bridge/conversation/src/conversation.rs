pub use agentelier_conversation::{
    ContextLimits, SessionCreate, SessionId, TaskAction, TaskActionOutcome, TaskActionReceipt,
    TaskId,
};
use agentelier_prompts::{InstructionMessage, InstructionSnapshot};
use std::sync::Arc;

#[derive(Clone)]
pub struct SessionSettings {
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub image_input: Option<bool>,
    pub context_limits: Box<ContextLimits>,
    pub instructions: Arc<InstructionSnapshot>,
    pub working_directory: std::path::PathBuf,
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
    CreateAndSend {
        create: SessionCreate,
        input: String,
    },
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
        audio: Option<agentelier_conversation::AudioId>,
        transcript: Option<agentelier_conversation::TranscriptId>,
    },
    Retry {
        session: SessionId,
        turn: u64,
    },
    ContinueWithoutCompaction {
        session: SessionId,
        turn: u64,
        failed_request: u32,
    },
}
#[derive(Clone)]
pub enum TurnStatus {
    Running,
    Completed,
    Cancelled,
    Failed(String),
    Limited,
}
#[derive(Clone)]
pub enum HistoryItem {
    AgentResult,
    Project { publication: u64 },
    Compaction,
    User,
    FunctionCall { call_id: String, name: String },
    FunctionResult { call_id: String },
    Assistant { phase: Option<String> },
    Reasoning { encrypted: bool },
}
#[derive(Clone)]
pub struct MessageView {
    pub id: String,
    pub output_index: usize,
    pub request_index: u32,
    pub phase: Option<String>,
    pub text: String,
}
#[derive(Clone)]
pub struct ToolView {
    pub collaboration: bool,
    pub request_index: u32,
    pub id: String,
    pub output_index: usize,
    pub call_id: String,
    pub name: String,
    pub source: Option<String>,
    pub arguments: String,
    pub arguments_truncated: bool,
    pub status: String,
    pub output: Option<String>,
    pub image: Option<ToolImageView>,
    pub retained: bool,
}

#[derive(Clone)]
pub struct ToolImageView {
    pub digest: String,
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub exportable: bool,
}

pub struct SessionTask {
    pub id: String,
    pub instance: Option<String>,
    pub turn: u64,
    pub request: u32,
    pub call_id: String,
    pub tool: String,
    pub status: String,
    pub source: &'static str,
    pub argument_bytes: Option<usize>,
    pub delivery: &'static str,
    pub cancelling: bool,
}

pub struct SessionTasks {
    pub session: SessionId,
    pub revision: u64,
    pub tasks: Vec<SessionTask>,
    pub background_paused: bool,
    pub queue_paused: bool,
    pub external_unconfirmed: Vec<String>,
}

#[derive(Clone)]
pub struct SessionProcess {
    pub id: String,
    pub instance: SessionId,
    pub turn: u64,
    pub call_id: String,
    pub phase: &'static str,
    pub exit_code: Option<i64>,
    pub readable: bool,
    pub truncated: bool,
    pub decoding_replaced: bool,
}

pub struct SessionProcesses {
    pub session: SessionId,
    pub revision: u64,
    pub processes: Vec<SessionProcess>,
}
#[derive(Clone, Copy)]
pub enum SteerState {
    Accepted,
    Applied,
    NotApplied,
}
#[derive(Clone)]
pub struct SteerView {
    pub id: u64,
    pub text: String,
    pub state: SteerState,
    pub applied_request: Option<u32>,
}
pub enum InputDecision {
    Accepted,
    Applied,
    Rejected(String),
}
pub struct InputReceipt {
    pub id: u64,
    pub decision: InputDecision,
}
#[derive(Clone)]
pub struct ProcessCleanup {
    pub session_id: String,
    pub cleanup_confirmed: bool,
    pub output: String,
}
#[derive(Clone)]
pub struct RequestMemory {
    pub publication_id: String,
    pub hash: String,
    pub text: String,
    pub role: String,
    pub input_index: usize,
}

#[derive(Clone)]
pub struct RequestView {
    pub transcript: Option<(usize, TranscriptReferenceView)>,
    pub response_accepted: Option<bool>,
    pub additional_tools_in_input: bool,
    pub memory: Option<RequestMemory>,
    pub skills: Vec<(usize, String)>,
    pub sending_started: bool,
    pub response_status: Option<u16>,
    pub request: u32,
    pub window: u64,
    pub publication: Option<u64>,
    pub items: Vec<HistoryItem>,
    pub base_hashes: Vec<String>,
    pub payload_hash: String,
    pub payload_bytes: usize,
}

#[derive(Clone)]
pub struct TurnView {
    pub result_continuation: bool,
    pub compaction_continuation: Option<u32>,
    pub requests: Vec<RequestView>,
    pub id: u64,
    pub operation_id: u64,
    pub input_id: u64,
    pub is_retry: bool,
    pub input: String,
    pub audio: Option<AudioAttachmentView>,
    pub transcript: Option<TranscriptReferenceView>,
    pub messages: Vec<MessageView>,
    pub status: TurnStatus,
    pub request_items: Vec<HistoryItem>,
    pub request_index: u32,
    pub committed_steps: u32,
    pub tools: Vec<ToolView>,
    pub phase: String,
    pub steers: Vec<SteerView>,
    pub request_instructions: Vec<InstructionMessage>,
    pub process_cleanup: Vec<ProcessCleanup>,
}
#[derive(Clone)]
pub struct QueuedInput {
    pub operation_id: u64,
    pub input_id: u64,
    pub text: String,
    pub transcript: Option<TranscriptReferenceView>,
}
#[derive(Clone)]
pub enum SessionAction {
    Enqueue {
        session: SessionId,
        text: String,
        transcript: Option<agentelier_conversation::TranscriptId>,
    },
    Remove {
        session: SessionId,
        operation: u64,
    },
    Resume {
        session: SessionId,
    },
}
pub enum QueueDecision {
    Enqueued,
    Removed { operation: u64 },
    AlreadyStarted { operation: u64 },
    Resumed,
    Rejected(String),
}
pub struct QueueEvent {
    pub id: u64,
    pub view: Option<SessionView>,
    pub decision: QueueDecision,
}
#[derive(Clone)]
pub struct ContextView {
    pub window: u64,
    pub limits: ContextLimits,
    pub tokens: u64,
    pub estimated_tokens: u64,
    pub server_anchor: bool,
    pub threshold_reached: bool,
    pub latest_usage: Option<(u64, u64, u64)>,
    pub covered_items: Option<usize>,
}
#[derive(Clone)]
pub struct SessionView {
    pub codex_identity: Option<(String, String)>,
    pub project_publications: Arc<Vec<ProjectPublicationView>>,
    pub project_context: Box<ProjectContextView>,
    pub context: Box<ContextView>,
    pub notice: Option<String>,
    pub id: SessionId,
    pub settings: SessionSettings,
    pub turn: Option<Box<TurnView>>,
    pub previous_turns: Vec<TurnView>,
    pub audio_draft: Option<AudioAttachmentView>,
    pub transcriptions: Vec<TranscriptionView>,
    pub committed_items: Vec<HistoryItem>,
    pub execution_blocked: bool,
    pub tool_catalog: Vec<(String, String)>,
    pub queue: Vec<QueuedInput>,
    pub queue_paused: bool,
}

#[derive(Clone)]
pub struct AudioAttachmentView {
    pub id: agentelier_conversation::AudioId,
    pub name: String,
    pub mime: &'static str,
    pub digest: String,
    pub encoded_bytes: usize,
    pub duration_ms: u64,
}

impl From<agentelier_conversation::AudioAttachment> for AudioAttachmentView {
    fn from(audio: agentelier_conversation::AudioAttachment) -> Self {
        Self {
            id: audio.id,
            name: audio.name,
            mime: audio.format.mime(),
            digest: audio.digest,
            encoded_bytes: audio.encoded_bytes,
            duration_ms: audio.duration_ms,
        }
    }
}

#[derive(Clone)]
pub struct TranscriptionView {
    pub id: agentelier_conversation::TranscriptId,
    pub audio: agentelier_conversation::AudioId,
    pub digest: String,
    pub engine: &'static str,
    pub outcome: TranscriptionOutcomeView,
}

#[derive(Clone)]
pub struct TranscriptReferenceView {
    pub session: SessionId,
    pub audio: SessionId,
    pub transcript: SessionId,
    pub filename: String,
    pub engine: String,
    pub audio_sha256: String,
    pub text_sha256: String,
}

impl From<agentelier_conversation::TranscriptReference> for TranscriptReferenceView {
    fn from(value: agentelier_conversation::TranscriptReference) -> Self {
        Self {
            session: value.session,
            audio: value.audio,
            transcript: value.transcript,
            filename: value.filename,
            engine: value.engine,
            audio_sha256: value.audio_sha256,
            text_sha256: value.text_sha256,
        }
    }
}

#[derive(Clone)]
pub enum TranscriptionOutcomeView {
    Completed(String),
    Failed(String),
    Cancelled,
    Interrupted,
}

impl From<&agentelier_conversation::AudioTranscription> for TranscriptionView {
    fn from(result: &agentelier_conversation::AudioTranscription) -> Self {
        use agentelier_conversation::{TranscriptionEngine, TranscriptionOutcome};
        Self {
            id: result.source.id,
            audio: result.source.audio,
            digest: result.source.digest.clone(),
            engine: match result.source.engine {
                TranscriptionEngine::Codex => "Codex",
            },
            outcome: match &result.outcome {
                TranscriptionOutcome::Completed(text) => {
                    TranscriptionOutcomeView::Completed(text.clone())
                }
                TranscriptionOutcome::Failed(error) => {
                    TranscriptionOutcomeView::Failed(error.clone())
                }
                TranscriptionOutcome::Cancelled => TranscriptionOutcomeView::Cancelled,
                TranscriptionOutcome::Interrupted => TranscriptionOutcomeView::Interrupted,
            },
        }
    }
}
#[derive(Clone)]
pub enum ProjectCollectionStatus {
    NotCollected,
    Loaded,
    Failed(String),
}
#[derive(Clone)]
pub struct ProjectRuleSource {
    pub path: std::path::PathBuf,
    pub scope: std::path::PathBuf,
    pub text: String,
    pub hash: String,
}
#[derive(Clone)]
pub struct ProjectSnapshotView {
    pub working_directory: std::path::PathBuf,
    pub project_root: Option<std::path::PathBuf>,
    pub sources: Vec<ProjectRuleSource>,
    pub source_bytes: usize,
    pub hash: String,
}
#[derive(Clone)]
pub struct ProjectContextView {
    pub status: ProjectCollectionStatus,
    pub snapshot: Option<ProjectSnapshotView>,
}

#[derive(Clone)]
pub struct ProjectPublicationView {
    pub id: u64,
    pub window: u64,
    pub kind: String,
    pub snapshot: ProjectSnapshotView,
    pub rendered: Option<String>,
}

fn project_context(state: agentelier_conversation::ProjectContextState) -> ProjectContextView {
    use agentelier_conversation::ProjectContextState;
    let (status, snapshot) = match state {
        ProjectContextState::NotCollected => (ProjectCollectionStatus::NotCollected, None),
        ProjectContextState::Loaded(snapshot) => (ProjectCollectionStatus::Loaded, Some(snapshot)),
        ProjectContextState::Failed {
            message,
            last_success,
        } => (ProjectCollectionStatus::Failed(message), last_success),
    };
    ProjectContextView {
        status,
        snapshot: snapshot.map(|snapshot| ProjectSnapshotView {
            working_directory: snapshot.working_directory.clone(),
            project_root: snapshot.project_root.clone(),
            source_bytes: snapshot.source_bytes,
            hash: snapshot.hash.clone(),
            sources: snapshot
                .sources
                .iter()
                .map(|source| ProjectRuleSource {
                    path: source.path.clone(),
                    scope: source.scope.clone(),
                    text: source.text.clone(),
                    hash: source.hash.clone(),
                })
                .collect(),
        }),
    }
}

pub enum ConversationEvent {
    AudioDraft {
        session: SessionId,
        result: Result<Option<AudioAttachmentView>, String>,
    },
    AudioTranscription {
        session: SessionId,
        result: Result<TranscriptionView, String>,
    },
    IdleUpdated(SessionView),
    Processes(SessionProcesses),
    TaskAction(TaskActionReceipt),
    Tasks(SessionTasks),
    GlobalTasks(crate::GlobalTasks),
    TaskNotification(crate::TaskNotification),
    ArchiveCleanup(Vec<String>),
    Deleted {
        session: SessionId,
        cleanup_error: Option<String>,
        was_open: bool,
    },
    MetadataSaved {
        session: SessionId,
        result: Result<ArchiveEntry, String>,
    },
    Archives(Vec<ArchiveEntry>),
    Projects(Result<crate::ProjectCatalog, String>),
    ProjectSaved(Result<(), String>),
    Opened(SessionView),
    ArchiveFailed(String),
    Created(SessionView),
    CreatedForSend(SessionView),
    WorkingDirectorySaved(SessionView),
    SessionModelSaved {
        session: SessionId,
        result: Result<SessionView, String>,
    },
    Accepted(SessionView),
    Updated(SessionView),
    Finished(SessionView),
    Rejected(String),
    InputReceipt(InputReceipt),
    Queue(QueueEvent),
    Started(SessionView),
}

#[derive(Clone)]
pub struct ArchiveEntry {
    pub archived: bool,
    pub id: SessionId,
    pub title: String,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub last_sequence: u64,
    pub error: Option<String>,
}

impl ConversationEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Deleted { .. }
                | Self::Created(_)
                | Self::WorkingDirectorySaved(_)
                | Self::Finished(_)
                | Self::Rejected(_)
                | Self::Opened(_)
                | Self::ArchiveFailed(_)
        )
    }
    pub fn is_progress(&self) -> bool {
        matches!(self, Self::Updated(_))
    }
}

pub fn request(request: ConversationRequest) -> agentelier_conversation::ConversationRequest {
    match request {
        ConversationRequest::ProcessResults { session } => {
            agentelier_conversation::ConversationRequest::ProcessResults { session }
        }
        ConversationRequest::Delete { session } => {
            agentelier_conversation::ConversationRequest::Delete { session }
        }
        ConversationRequest::Open { session } => {
            agentelier_conversation::ConversationRequest::Open { session }
        }
        ConversationRequest::Create(create) => {
            agentelier_conversation::ConversationRequest::Create(create)
        }
        ConversationRequest::CreateAndSend { create, input } => {
            agentelier_conversation::ConversationRequest::CreateAndSend { create, input }
        }
        ConversationRequest::Reset { session } => {
            agentelier_conversation::ConversationRequest::Reset { session }
        }
        ConversationRequest::SetWorkingDirectory { session, directory } => {
            agentelier_conversation::ConversationRequest::SetWorkingDirectory { session, directory }
        }
        ConversationRequest::Retry { session, turn } => {
            agentelier_conversation::ConversationRequest::Retry {
                session,
                turn: agentelier_conversation::TurnId(turn),
            }
        }
        ConversationRequest::ContinueWithoutCompaction {
            session,
            turn,
            failed_request,
        } => agentelier_conversation::ConversationRequest::ContinueWithoutCompaction {
            session,
            turn: agentelier_conversation::TurnId(turn),
            failed_request,
        },
        ConversationRequest::Send {
            session,
            input,
            audio,
            transcript,
        } => agentelier_conversation::ConversationRequest::Send {
            session,
            input,
            audio,
            transcript,
        },
    }
}

pub fn view(session: agentelier_conversation::SessionView) -> SessionView {
    SessionView {
        project_publications: Arc::new(
            session
                .project_publications
                .iter()
                .map(|publication| {
                    use agentelier_conversation::PublicationKind;
                    ProjectPublicationView {
                        id: publication.id,
                        window: publication.window,
                        kind: match publication.kind {
                            PublicationKind::Initial => "首次发布",
                            PublicationKind::Replacement => "完整替换",
                            PublicationKind::Removal => "清除",
                            PublicationKind::Republished => "压缩后重新发布",
                            PublicationKind::RepublishedRemoval => "压缩后重新发布清除状态",
                        }
                        .into(),
                        snapshot: ProjectSnapshotView {
                            working_directory: publication.snapshot.working_directory.clone(),
                            project_root: publication.snapshot.project_root.clone(),
                            source_bytes: publication.snapshot.source_bytes,
                            hash: publication.snapshot.hash.clone(),
                            sources: publication
                                .snapshot
                                .sources
                                .iter()
                                .map(|source| ProjectRuleSource {
                                    path: source.path.clone(),
                                    scope: source.scope.clone(),
                                    text: source.text.clone(),
                                    hash: source.hash.clone(),
                                })
                                .collect(),
                        },
                        rendered: publication.item.as_ref().map(|item| item.text.clone()),
                    }
                })
                .collect(),
        ),
        project_context: Box::new(project_context(*session.project_context)),
        context: Box::new(ContextView {
            window: session.context.window,
            tokens: session.context.tokens,
            estimated_tokens: session.context.estimated_tokens,
            server_anchor: session.context.anchor.is_some(),
            threshold_reached: session.context.limits.reached(session.context.tokens),
            latest_usage: session.context.latest.as_ref().map(|fact| {
                (
                    fact.usage.input_tokens,
                    fact.usage.output_tokens,
                    fact.usage.total_tokens,
                )
            }),
            covered_items: session
                .context
                .anchor
                .as_ref()
                .map(|anchor| anchor.covered_items),
            limits: session.context.limits,
        }),
        codex_identity: session
            .settings
            .codex
            .map(|identity| (identity.subject, identity.workspace_id)),
        notice: session.notice,
        execution_blocked: session.execution_blocked,
        tool_catalog: session.tool_catalog,
        queue: session
            .queue
            .into_iter()
            .map(|input| QueuedInput {
                operation_id: input.operation_id,
                input_id: input.input_id.0,
                text: input.text,
                transcript: input.transcript.map(Into::into),
            })
            .collect(),
        queue_paused: session.queue_paused,
        id: session.id,
        settings: SessionSettings {
            slug: session.settings.slug,
            display_name: session.settings.display_name,
            effort: session.settings.effort,
            image_input: session.settings.image_input,
            context_limits: session.settings.context_limits,
            instructions: session.settings.instructions,
            working_directory: session.settings.working_directory,
        },
        turn: session.turn.map(|turn| Box::new(turn_view(*turn))),
        previous_turns: session.previous_turns.into_iter().map(turn_view).collect(),
        audio_draft: session.audio_draft.map(Into::into),
        transcriptions: session.transcriptions.iter().map(Into::into).collect(),
        committed_items: session
            .committed_items
            .into_iter()
            .map(history_item)
            .collect(),
    }
}

fn history_item(item: agentelier_conversation::ProtocolItemSummary) -> HistoryItem {
    match item {
        agentelier_conversation::ProtocolItemSummary::Compaction => HistoryItem::Compaction,
        agentelier_conversation::ProtocolItemSummary::Project { publication, .. } => {
            HistoryItem::Project { publication }
        }
        agentelier_conversation::ProtocolItemSummary::User => HistoryItem::User,
        agentelier_conversation::ProtocolItemSummary::AgentResult => HistoryItem::AgentResult,
        agentelier_conversation::ProtocolItemSummary::FunctionCall { call_id, name } => {
            HistoryItem::FunctionCall { call_id, name }
        }
        agentelier_conversation::ProtocolItemSummary::FunctionResult { call_id } => {
            HistoryItem::FunctionResult { call_id }
        }
        agentelier_conversation::ProtocolItemSummary::Assistant { phase } => {
            HistoryItem::Assistant { phase }
        }
        agentelier_conversation::ProtocolItemSummary::Reasoning { encrypted } => {
            HistoryItem::Reasoning { encrypted }
        }
    }
}
fn turn_view(turn: agentelier_conversation::TurnView) -> TurnView {
    TurnView {
        result_continuation: turn.result_continuation,
        compaction_continuation: turn.compaction_continuation.map(|source| source.request),
        requests: turn
            .requests
            .into_iter()
            .map(|fact| RequestView {
                response_accepted: fact.response_accepted,
                additional_tools_in_input: fact.projection.additional_tools_in_input(),
                memory: fact.projection.memory.map(|material| RequestMemory {
                    publication_id: material.snapshot.publication_id.to_string(),
                    hash: material.snapshot.hash,
                    text: material.text,
                    role: material.role,
                    input_index: material.input_index,
                }),
                transcript: fact
                    .projection
                    .transcript
                    .map(|item| (item.input_index, item.reference.into())),
                skills: fact
                    .projection
                    .skill_materials
                    .into_iter()
                    .map(|material| {
                        (
                            material.input_index,
                            if material.is_selected() {
                                "user · 显式 Skill"
                            } else {
                                "developer · Skills 目录"
                            }
                            .into(),
                        )
                    })
                    .collect(),
                sending_started: fact.sending_started,
                response_status: fact.response_status,
                request: fact.request,
                window: fact.window,
                publication: fact.publication,
                items: fact
                    .projection
                    .items
                    .into_iter()
                    .map(history_item)
                    .collect(),
                base_hashes: fact.projection.base_hashes,
                payload_hash: fact.projection.payload_hash,
                payload_bytes: fact.projection.payload_bytes,
            })
            .collect(),
        steers: turn
            .steers
            .into_iter()
            .map(|input| SteerView {
                id: input.id,
                text: input.text,
                applied_request: input.applied_request,
                state: match input.state {
                    agentelier_conversation::SteerState::Accepted => SteerState::Accepted,
                    agentelier_conversation::SteerState::Applied => SteerState::Applied,
                    agentelier_conversation::SteerState::NotApplied => SteerState::NotApplied,
                },
            })
            .collect(),
        request_index: turn.request_index,
        committed_steps: turn.committed_steps,
        phase: match turn.phase {
            agentelier_conversation::TaskPhase::CompactingContext => "compaction",
            agentelier_conversation::TaskPhase::LoadingProjectContext => "project_context",
            agentelier_conversation::TaskPhase::RequestingModel => "model",
            agentelier_conversation::TaskPhase::ExecutingTool => "tool",
            agentelier_conversation::TaskPhase::ReapingProcesses => "cleanup",
        }
        .into(),
        tools: turn
            .tools
            .into_iter()
            .map(|tool| ToolView {
                collaboration: tool.collaboration,
                request_index: tool.request_index,
                id: tool.id,
                output_index: tool.output_index,
                call_id: tool.call_id,
                name: tool.name,
                source: tool.source,
                arguments: tool.arguments,
                arguments_truncated: tool.arguments_truncated,
                output: tool.output,
                image: tool.image.map(|image| {
                    let descriptor = image.descriptor();
                    ToolImageView {
                        digest: descriptor.digest.clone(),
                        width: descriptor.width,
                        height: descriptor.height,
                        mime: descriptor.mime.clone(),
                        exportable: matches!(
                            image,
                            agentelier_conversation::ToolImage::Generated(_)
                        ),
                    }
                }),
                retained: tool.retained,
                status: tool_status(tool.status).into(),
            })
            .collect(),
        id: turn.id.0,
        input_id: turn.input_id.0,
        is_retry: turn.is_retry,
        operation_id: turn.operation_id,
        input: turn.input,
        audio: turn.audio.map(Into::into),
        transcript: turn.transcript.map(Into::into),
        messages: turn
            .messages
            .into_iter()
            .map(|message| MessageView {
                id: message.id,
                output_index: message.output_index,
                request_index: message.request_index,
                phase: message.phase,
                text: message
                    .content
                    .into_iter()
                    .map(|part| part.text)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            })
            .collect(),
        request_items: turn.request_items.into_iter().map(history_item).collect(),
        request_instructions: turn.request_instructions,
        process_cleanup: turn
            .process_cleanup
            .into_iter()
            .map(|record| ProcessCleanup {
                session_id: record.session_id,
                cleanup_confirmed: record.cleanup_confirmed,
                output: record.output,
            })
            .collect(),
        status: match turn.status {
            agentelier_conversation::TurnStatus::Limited => TurnStatus::Limited,
            agentelier_conversation::TurnStatus::Running => TurnStatus::Running,
            agentelier_conversation::TurnStatus::Completed => TurnStatus::Completed,
            agentelier_conversation::TurnStatus::Cancelled => TurnStatus::Cancelled,
            agentelier_conversation::TurnStatus::Failed(reason) => TurnStatus::Failed(reason),
        },
    }
}

fn tool_status(status: agentelier_conversation::ToolStatus) -> &'static str {
    use agentelier_conversation::ToolStatus;
    match status {
        ToolStatus::Unconfirmed => "unconfirmed",
        ToolStatus::ExternalUnconfirmed => "external_unconfirmed",
        ToolStatus::Receiving => "receiving",
        ToolStatus::Pending => "pending",
        ToolStatus::Running => "running",
        ToolStatus::Succeeded => "succeeded",
        ToolStatus::Failed => "failed",
        ToolStatus::TimedOut => "timed_out",
        ToolStatus::Cancelled => "cancelled",
        ToolStatus::NotExecuted => "not_executed",
    }
}

pub fn convert(event: agentelier_conversation::ConversationEvent) -> ConversationEvent {
    match event {
        agentelier_conversation::ConversationEvent::IdleUpdated(state) => {
            ConversationEvent::IdleUpdated(view(state))
        }
        agentelier_conversation::ConversationEvent::AudioDraft { session, result } => {
            ConversationEvent::AudioDraft {
                session,
                result: result.map(|draft| draft.map(Into::into)),
            }
        }
        agentelier_conversation::ConversationEvent::AudioTranscription { session, result } => {
            ConversationEvent::AudioTranscription {
                session,
                result: result.map(|item| (&item).into()),
            }
        }
        agentelier_conversation::ConversationEvent::TaskAction(receipt) => {
            ConversationEvent::TaskAction(receipt)
        }
        agentelier_conversation::ConversationEvent::Processes(snapshot) => {
            ConversationEvent::Processes(SessionProcesses {
                session: snapshot.session,
                revision: snapshot.revision,
                processes: snapshot
                    .processes
                    .into_iter()
                    .map(|p| SessionProcess {
                        id: p.session_id,
                        instance: p.origin.instance,
                        turn: p.origin.turn,
                        call_id: p.origin.call_id,
                        phase: p.phase.as_str(),
                        exit_code: p.exit_code,
                        readable: p.readable,
                        truncated: p.truncated,
                        decoding_replaced: p.decoding_replaced,
                    })
                    .collect(),
            })
        }
        agentelier_conversation::ConversationEvent::GlobalTasks(snapshot) => {
            ConversationEvent::GlobalTasks(snapshot)
        }
        agentelier_conversation::ConversationEvent::TaskNotification(notice) => {
            ConversationEvent::TaskNotification(notice)
        }
        agentelier_conversation::ConversationEvent::Tasks(update) => {
            ConversationEvent::Tasks(SessionTasks {
                session: update.session,
                revision: update.revision,
                background_paused: update.background_paused,
                queue_paused: update.queue_paused,
                external_unconfirmed: update.external_unconfirmed,
                tasks: update
                    .tasks
                    .into_iter()
                    .map(|task| SessionTask {
                        id: task.task.0.to_string(),
                        instance: task.instance.map(|instance| instance.to_string()),
                        turn: task.turn.0,
                        request: task.request,
                        call_id: task.call_id,
                        tool: task.tool,
                        status: tool_status(task.status).into(),
                        source: task.source,
                        argument_bytes: task.argument_bytes,
                        cancelling: task.cancelling,
                        delivery: match task.delivery {
                            agentelier_conversation::TaskDelivery::AwaitingResult => "awaiting",
                            agentelier_conversation::TaskDelivery::Saved => "saved",
                            agentelier_conversation::TaskDelivery::Appended => "pending",
                            agentelier_conversation::TaskDelivery::Delivered => "delivered",
                        },
                    })
                    .collect(),
            })
        }
        agentelier_conversation::ConversationEvent::ArchiveCleanup(errors) => {
            ConversationEvent::ArchiveCleanup(errors)
        }
        agentelier_conversation::ConversationEvent::Deleted {
            session,
            cleanup_error,
            was_open,
        } => ConversationEvent::Deleted {
            session,
            cleanup_error,
            was_open,
        },
        agentelier_conversation::ConversationEvent::MetadataSaved { session, result } => {
            ConversationEvent::MetadataSaved {
                session,
                result: result.map(|entry| {
                    let summary = entry.summary;
                    ArchiveEntry {
                        id: summary.session,
                        archived: summary.archived,
                        title: summary.title,
                        created_at_ms: summary.created_at_ms,
                        last_activity_ms: summary.last_activity_ms,
                        last_sequence: summary.last_sequence,
                        error: entry.error,
                    }
                }),
            }
        }
        agentelier_conversation::ConversationEvent::Projects(catalog) => {
            ConversationEvent::Projects(catalog)
        }
        agentelier_conversation::ConversationEvent::ProjectSaved(result) => {
            ConversationEvent::ProjectSaved(result)
        }
        agentelier_conversation::ConversationEvent::Archives(entries) => {
            ConversationEvent::Archives(
                entries
                    .into_iter()
                    .map(|entry| ArchiveEntry {
                        id: entry.summary.session,
                        archived: entry.summary.archived,
                        title: entry.summary.title,
                        created_at_ms: entry.summary.created_at_ms,
                        last_activity_ms: entry.summary.last_activity_ms,
                        last_sequence: entry.summary.last_sequence,
                        error: entry.error,
                    })
                    .collect(),
            )
        }
        agentelier_conversation::ConversationEvent::Opened(session) => {
            ConversationEvent::Opened(view(session))
        }
        agentelier_conversation::ConversationEvent::ArchiveFailed(reason) => {
            ConversationEvent::ArchiveFailed(reason)
        }
        agentelier_conversation::ConversationEvent::Started(session) => {
            ConversationEvent::Started(view(session))
        }
        agentelier_conversation::ConversationEvent::Queue(event) => {
            ConversationEvent::Queue(QueueEvent {
                id: event.id,
                view: event.view.map(view),
                decision: match event.decision {
                    agentelier_conversation::QueueDecision::Enqueued => QueueDecision::Enqueued,
                    agentelier_conversation::QueueDecision::Removed { operation } => {
                        QueueDecision::Removed { operation }
                    }
                    agentelier_conversation::QueueDecision::AlreadyStarted { operation } => {
                        QueueDecision::AlreadyStarted { operation }
                    }
                    agentelier_conversation::QueueDecision::Resumed => QueueDecision::Resumed,
                    agentelier_conversation::QueueDecision::Rejected(reason) => {
                        QueueDecision::Rejected(reason)
                    }
                },
            })
        }
        agentelier_conversation::ConversationEvent::InputReceipt(receipt) => {
            ConversationEvent::InputReceipt(InputReceipt {
                id: receipt.id,
                decision: match receipt.decision {
                    agentelier_conversation::InputDecision::Accepted => InputDecision::Accepted,
                    agentelier_conversation::InputDecision::Applied => InputDecision::Applied,
                    agentelier_conversation::InputDecision::Rejected(reason) => {
                        InputDecision::Rejected(reason)
                    }
                },
            })
        }
        agentelier_conversation::ConversationEvent::Created(session) => {
            ConversationEvent::Created(view(session))
        }
        agentelier_conversation::ConversationEvent::CreatedForSend(session) => {
            ConversationEvent::CreatedForSend(view(session))
        }
        agentelier_conversation::ConversationEvent::WorkingDirectorySaved(session) => {
            ConversationEvent::WorkingDirectorySaved(view(session))
        }
        agentelier_conversation::ConversationEvent::SessionModelSaved { session, result } => {
            ConversationEvent::SessionModelSaved {
                session,
                result: result.map(view),
            }
        }
        agentelier_conversation::ConversationEvent::Accepted(session) => {
            ConversationEvent::Accepted(view(session))
        }
        agentelier_conversation::ConversationEvent::Updated(session) => {
            ConversationEvent::Updated(view(session))
        }
        agentelier_conversation::ConversationEvent::Finished(session) => {
            ConversationEvent::Finished(view(session))
        }
        agentelier_conversation::ConversationEvent::Rejected(reason) => {
            ConversationEvent::Rejected(reason)
        }
    }
}

pub fn steer_input(
    id: u64,
    session: SessionId,
    turn: u64,
    operation_id: u64,
    text: String,
) -> agentelier_conversation::SteerInput {
    agentelier_conversation::SteerInput {
        id,
        session,
        turn: agentelier_conversation::TurnId(turn),
        operation_id,
        text,
    }
}

pub fn session_command(id: u64, action: SessionAction) -> agentelier_conversation::SessionCommand {
    match action {
        SessionAction::Enqueue {
            session,
            text,
            transcript,
        } => agentelier_conversation::SessionCommand::Enqueue {
            id,
            session,
            text,
            transcript,
        },
        SessionAction::Remove { session, operation } => {
            agentelier_conversation::SessionCommand::Remove {
                id,
                session,
                operation,
            }
        }
        SessionAction::Resume { session } => {
            agentelier_conversation::SessionCommand::Resume { id, session }
        }
    }
}
