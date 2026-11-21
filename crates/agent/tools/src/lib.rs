#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod blocking;
mod catalog;
pub mod desktop;
mod environment;
mod execution;
mod image;
mod output;
mod patch;
mod payload;
mod process;
mod sessions;
mod task_control;

pub use environment::WorkingDirectory;
pub use image::from_mcp_bytes;

pub use catalog::{
    ToolCatalog, ToolContract, ToolDefinition, ToolExecution, ToolMode, ToolRegistration,
    ToolSource,
};
pub use execution::{MAX_TOOL_BODY_BYTES, ToolFailure, ToolResult, ToolStatus, execute};

pub use admission::{ToolAccess, ToolAdmission, ToolPermit};
pub use execution::{ExecutionContext, ToolUserTurn};
pub use payload::{
    GENERATE_IMAGE_TOOL, GENERATED_IMAGE_MODEL, ImageDescriptor, ImagePayload,
    MAX_ENCODED_IMAGE_BYTES, MAX_GENERATION_PROMPT_CHARS, MAX_ORIGINAL_IMAGE_BYTES,
    MAX_SESSION_IMAGE_BYTES, MAX_SESSION_ORIGINAL_IMAGE_BYTES, OriginalImage, ToolPayload,
};
mod process_facts;
pub use process_facts::{
    ProcessChange, ProcessFact, ProcessOrigin, ProcessPhase, ProcessView, SaveProcessFact,
};
pub use sessions::{
    CleanupReport, MAX_PROCESS_SESSIONS, ProcessCleanup, ProcessScope, ProcessSessions,
};
pub use task_control::{
    MAX_TASK_PAGE, TaskControl, TaskControlRequest, TaskList, TaskTargets, TaskWait, WaitMode,
};
mod admission;
