#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod conversation;
mod preparation;
mod reasoning;

pub use conversation::{
    ChatHistoryItem, ChatMessage, ChatReplacement, ChatSession, ChatState, ChatTurn, ChatTurnStatus,
};
pub use preparation::{ChatUiHost, ModelChoice, ModelPhase, ModelState, install};
pub use reasoning::ReasoningSetting;
