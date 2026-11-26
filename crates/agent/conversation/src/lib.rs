#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod active;
mod agent_registry;
mod background;
mod behavior;
pub use agent_registry::{
    AgentId, AgentIdentity, AgentNode, AgentOrigin, AgentPath, AgentRegistry, AgentRegistryError,
    AgentReservation, AgentResidency, AgentRuntimeId, AgentState, AgentStop, AgentStopSource,
    AgentThreadId, ChildRegistration,
};
mod project_catalog;
mod project_context;
pub use agentelier_llm_model::PublicationKind;
pub use project_catalog::{Project, ProjectCatalog, ProjectChange, ProjectId, ProjectView};
pub use project_context::{PreparedRequestFact, ProjectCandidate, ProjectContextState};
mod archive;
mod audio;
mod child_config;
mod native_scene;
pub use child_config::{
    AgentCreationRecord, ChildConfigSnapshot, ChildExecutionPolicy, ChildMaterials,
    ChildPreparation, ForkBoundary, ForkMode, PreparedChild,
};
mod control;
mod conversation;
mod expression;
mod mailbox;
pub use mailbox::{
    AgentMessage, AgentMessageId, ClaimedMessage, DeliveryMode, MAX_AGENT_MESSAGE_CHARS,
    MailboxEntry, MailboxEvent, Mailboxes, MessageClaim, MessageStatus, TaskCompletion,
};
mod global_tasks;
mod memory_evidence;
mod processes;
mod queue;
mod restore;
mod result_budget;
mod session_model;
mod task_actions;
mod task_policy;
mod tasks;
pub use archive::{ArchiveEntry, SessionMetadataChange};
pub use audio::{
    AudioAttachment, AudioFormat, AudioId, AudioTranscription, MAX_AUDIO_BYTES,
    MAX_AUDIO_NAME_CHARS, MAX_SESSION_AUDIO_BYTES, TranscriptId, TranscriptionEngine,
    TranscriptionOutcome, TranscriptionSource,
};
pub use global_tasks::{
    GlobalTask, GlobalTaskGroup, GlobalTaskProcess, GlobalTaskStatus, GlobalTasks,
    TaskNotification, TaskNotificationKind,
};
pub use memory_evidence::{MemorySourceTurn, memory_evidence};
pub use native_scene::{PublishedScene, SceneState, SceneStateReceipt};
pub use processes::ProcessLedger;
pub use restore::{RestoredChild, RestoredSession, restore, restore_child};
pub use session_model::{PreparedModelChange, StoredSessionModel};
pub use task_actions::{SessionProcesses, TaskAction, TaskActionOutcome, TaskActionReceipt};
pub use task_policy::{TaskPolicy, TaskPolicyChange};
pub use tasks::{
    ResponseDelivery, ResponseTasks, SessionTaskView, SessionTasks, TaskCancelReceipt,
    TaskCancelState, TaskCancellation, TaskCancellationPlan, TaskDelivery, TaskExecution, TaskId,
    TaskPage, TaskRegistration, TaskReport,
};

pub use archive::{
    GeneratedImage, InitialSession, RecordEvent, RecoveredResult, RecoveryEvidence,
    ResponseAcceptance, SessionRecord, SessionSummary, StoredImage, StoredSettings, StoredState,
    StoredTool,
};
pub use background::{BACKGROUND_RESULT_TOOL, BackgroundOutcome, BackgroundStatus, BackgroundTask};
pub use conversation::PreparedSession;

pub use conversation::{
    BackgroundBinding, CompactionContinuation, Conversation, ConversationEvent,
    ConversationFailure, ConversationRequest, MemoryWithdrawal, SessionCreate, SessionId,
    SessionKind, SessionLocation, SessionSettings, SessionView, TaskPhase, ToolImage, ToolView,
    TurnId, TurnMemory, TurnStatus, TurnView,
};

pub use agentelier_llm_model::ProtocolItemSummary;
pub use agentelier_llm_model::TranscriptReference;

pub use agentelier_tools::ToolStatus;

pub use active::ActiveTurn;
pub use control::{
    InputDecision, InputReceipt, MAX_PENDING_STEERS, SteerInput, SteerState, SteerView,
};

pub use queue::PreparedQueue;
pub use queue::{MAX_QUEUED_INPUTS, QueueDecision, QueueEvent, QueuedInput, SessionCommand};

mod metadata;
pub use metadata::session_title;

mod context;
pub use context::{ContextState, UsageAnchor, UsageFact};

pub use agentelier_llm_model::ContextLimits;

mod compaction;
pub use compaction::{CompactionCandidate, CompactionCheckpoint};
