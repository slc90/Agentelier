#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod active_input;
mod archives;
mod audio_attachment;
pub use archives::{ArchiveChoice, ArchiveState};
mod conversation;
mod global_tasks;
mod image_preview;
mod mcp;
mod mcp_materials;
mod memory;
mod memory_chat;
mod preparation;
mod project_context;
mod projects;
mod prompts;
mod queue;
mod reading;
mod reasoning;
mod request_inspection;
mod skills;
mod task_panel;
mod task_toast;
mod tool_timeline;
mod transcription;
mod working_directory;
mod workspace;
mod workspace_layout;

pub use conversation::{
    ChatHistoryItem, ChatMessage, ChatReplacement, ChatSession, ChatState, ChatTurn,
    ChatTurnStatus, SessionModelDefaults,
};
pub use preparation::{ChatUiHost, ModelChoice, ModelPhase, ModelState, install};
pub use reasoning::ReasoningSetting;
pub use workspace::{MAIN_WINDOW_WIDTH, MIN_MAIN_WINDOW_WIDTH, OpenSettings, workspace_title};
pub use workspace_layout::{CommitWorkspaceLayout, LayoutSide, WorkspaceLayout, WorkspaceRatios};

mod session_actions;
