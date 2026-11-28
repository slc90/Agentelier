#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod blocking;
mod catalog;
pub mod desktop;
pub mod ecs_access;
mod environment;
mod execution;
mod image;
mod output;
mod patch;
mod payload;
mod process;
mod script;
mod sessions;
mod task_control;

pub use environment::WorkingDirectory;
pub use image::{from_mcp_bytes, prepare_clipboard_image, prepare_local_image};
pub use script::widget;
pub use script::{ScriptOutput, ScriptSession};

pub use catalog::{
    ToolCatalog, ToolContract, ToolDefinition, ToolExecution, ToolMode, ToolRegistration,
    ToolSource,
};
pub use execution::{MAX_TOOL_BODY_BYTES, ToolFailure, ToolResult, ToolStatus, execute};

pub use admission::{ToolAccess, ToolAdmission, ToolPermit};
pub use execution::{ExecutionContext, ToolUserTurn};
pub use payload::{
    GENERATE_IMAGE_TOOL, GENERATED_IMAGE_MODEL, ImageDescriptor, ImagePayload,
    MAX_GENERATION_PROMPT_CHARS, MAX_ORIGINAL_IMAGE_BYTES, MAX_PROMPT_IMAGE_BYTES, OriginalImage,
    PUBLISH_AUDIO_TOOL, PUBLISH_SCENE_TOOL, ToolPayload, UPDATE_SCENE_TOOL,
};
mod process_facts;
pub use process_facts::{
    ProcessChange, ProcessFact, ProcessLifetime, ProcessOrigin, ProcessPhase, ProcessView,
    SaveProcessFact,
};
pub use sessions::{
    CleanupReport, MAX_PROCESS_SESSIONS, ProcessCleanup, ProcessScope, ProcessSessions,
};
pub use task_control::{
    MAX_TASK_PAGE, TaskControl, TaskControlRequest, TaskList, TaskTargets, TaskWait, WaitMode,
};
mod admission;
