#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod blocking;
mod catalog;
mod environment;
mod execution;
mod image;
mod output;
mod patch;
mod payload;
mod process;
mod sessions;

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
    ImageDescriptor, ImagePayload, MAX_ENCODED_IMAGE_BYTES, MAX_SESSION_IMAGE_BYTES, ToolPayload,
};
pub use sessions::{CleanupReport, ProcessCleanup, ProcessSessions};
mod admission;
