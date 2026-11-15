#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod conversation;

pub use conversation::{
    ConversationEvent, ConversationRequest, HistoryItem, MessageView, SessionSettings, SessionView,
    ToolView, TurnStatus, TurnView, convert, request,
};
