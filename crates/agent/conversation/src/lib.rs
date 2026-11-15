#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod active;
mod control;
mod conversation;
mod queue;

pub use conversation::{
    Conversation, ConversationEvent, ConversationFailure, ConversationRequest, SessionId,
    SessionSettings, SessionView, TaskPhase, ToolView, TurnId, TurnStatus, TurnView,
};

pub use agentelier_llm_model::ProtocolItemSummary;

pub use agentelier_tools::ToolStatus;

pub use active::ActiveTurn;
pub use control::{
    InputDecision, InputReceipt, MAX_PENDING_STEERS, RunnerEvent, SteerInput, SteerState, SteerView,
};

pub use queue::{MAX_QUEUED_INPUTS, QueueDecision, QueueEvent, QueuedInput, SessionCommand};
