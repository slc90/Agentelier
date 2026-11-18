#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod active_input;
mod archives;
pub use archives::{ArchiveChoice, ArchiveState};
mod conversation;
mod mcp;
mod mcp_materials;
mod preparation;
mod project_context;
mod prompts;
mod queue;
mod reading;
mod reasoning;
mod request_inspection;
mod tool_timeline;
mod working_directory;

pub use conversation::{
    ChatHistoryItem, ChatMessage, ChatReplacement, ChatSession, ChatState, ChatTurn, ChatTurnStatus,
};
pub use preparation::{ChatUiHost, ModelChoice, ModelPhase, ModelState, install};
pub use reasoning::ReasoningSetting;

mod session_actions;
