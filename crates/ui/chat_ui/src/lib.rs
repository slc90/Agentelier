#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod active_input;
mod archives;
mod audio_attachment;
mod audio_playback;
mod background_tasks;
pub use archives::{ArchiveChoice, ArchiveState};
mod composer;
mod composer_completion;
mod conversation;
mod generated_image;
mod global_tasks;
mod mcp;
mod mcp_materials;
mod memory;
mod memory_chat;
mod native_scene;
mod preparation;
mod project_context;
mod projects;
mod prompts;
mod queue;
mod reading;
mod request_inspection;
mod skills;
mod spinner;
mod task_toast;
mod transcription;
mod workspace;

pub use conversation::{
    ChatHistoryItem, ChatMessage, ChatSession, ChatState, ChatTurn, ChatTurnStatus,
    SessionModelDefaults,
};
pub use preparation::{ChatUiHost, ModelChoice, ModelState, install};
pub use workspace::{MAIN_WINDOW_WIDTH, OpenSettings, workspace_navigation};

mod session_actions;
