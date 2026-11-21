#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod access;
mod control;
mod dispatch;
mod runner;
mod task_control;
mod tool_events;

pub use access::{
    GeneratedImageFeedback, RunnerEvent, SavedGeneratedImage, TurnGeneratedImage, TurnImage,
    TurnRecord,
};
pub use dispatch::{
    SessionTaskAction, SessionTaskCommand, SessionTaskOutcome, ToolAuthorization, ToolBatch,
    ToolCommand, ToolDispatcher, ToolEvent, ToolJobRequest,
};
pub use runner::{IdleAction, run, run_idle};
pub use tool_events::ToolWork;
