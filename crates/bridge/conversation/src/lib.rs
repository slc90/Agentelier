#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod conversation;
pub use conversation::{
    AudioAttachmentView, ProjectCollectionStatus, ProjectContextView, ProjectPublicationView,
    ProjectRuleSource, ProjectSnapshotView, TranscriptionOutcomeView, TranscriptionView,
};

pub use conversation::{
    ConversationEvent, ConversationRequest, GeneratedImageView, HistoryItem, MessageView,
    RequestMemory, RequestView, SessionSettings, SessionView, ToolView, TurnStatus, TurnView,
    convert, request,
};

pub use conversation::{InputDecision, InputReceipt, SteerState, SteerView};

pub use conversation::steer_input;

pub use conversation::{QueueDecision, QueueEvent, QueuedInput, SessionAction, session_command};

pub use conversation::ArchiveEntry;
pub use conversation::SessionId;

pub use conversation::{ContextLimits, ContextView};
pub use conversation::{
    SessionProcess, SessionProcesses, TaskAction, TaskActionOutcome, TaskActionReceipt, TaskId,
};
