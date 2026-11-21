#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod active;
mod agent_registry;
pub use agent_registry::{
    AgentId, AgentIdentity, AgentNode, AgentOrigin, AgentPath, AgentRegistry, AgentRegistryError,
    AgentReservation, AgentResidency, AgentRuntimeId, AgentState, AgentThreadId, ChildRegistration,
};
mod project_context;
pub use agentelier_llm_model::PublicationKind;
pub use project_context::{PreparedRequestFact, ProjectCandidate, ProjectContextState};
mod archive;
mod audio;
mod child_config;
pub use child_config::{
    AgentCreationRecord, ChildConfigSnapshot, ChildExecutionPolicy, ChildPreparation, ForkBoundary,
    ForkMode, PreparedChild,
};
mod control;
mod conversation;
mod memory_evidence;
mod processes;
mod queue;
mod restore;
mod result_budget;
mod task_actions;
mod task_policy;
mod tasks;
pub use archive::ArchiveEntry;
pub use audio::{
    AudioAttachment, AudioFormat, AudioId, AudioTranscription, MAX_AUDIO_BYTES,
    MAX_AUDIO_NAME_CHARS, MAX_SESSION_AUDIO_BYTES, TranscriptId, TranscriptionEngine,
    TranscriptionOutcome, TranscriptionSource,
};
pub use memory_evidence::{MemorySourceTurn, memory_evidence};
pub use processes::ProcessLedger;
pub use restore::{RestoredSession, restore};
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
pub use conversation::PreparedSession;

pub use conversation::{
    CompactionContinuation, Conversation, ConversationEvent, ConversationFailure,
    ConversationRequest, MAX_TURN_REQUESTS, SessionId, SessionSettings, SessionView, TaskPhase,
    ToolView, TurnId, TurnStatus, TurnView,
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
