#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod active_input;
mod archives;
mod audio_attachment;
pub use archives::{ArchiveChoice, ArchiveState};
mod conversation;
mod image_preview;
mod mcp;
mod mcp_materials;
mod memory;
mod preparation;
mod project_context;
mod projects;
mod prompts;
mod queue;
mod reading;
mod reasoning;
mod request_inspection;
mod task_panel;
mod tool_timeline;
mod transcription;
mod working_directory;
mod workspace;

pub use conversation::{
    ChatHistoryItem, ChatMessage, ChatReplacement, ChatSession, ChatState, ChatTurn,
    ChatTurnStatus, SessionModelDefaults,
};
pub use preparation::{ChatUiHost, ModelChoice, ModelPhase, ModelState, install};
pub use reasoning::ReasoningSetting;
pub use workspace::{MAIN_WINDOW_WIDTH, MIN_MAIN_WINDOW_WIDTH, workspace_title};

mod session_actions;
