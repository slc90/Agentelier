#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod catalog;
mod diagnostic;
mod environment;
mod execution;
mod output;
mod process;
mod sessions;

pub use environment::WorkingDirectory;

pub use catalog::{ToolCatalog, ToolDefinition};
pub use execution::{MAX_TOOL_BODY_BYTES, ToolFailure, ToolResult, ToolStatus, execute};

pub use sessions::{CleanupReport, ExecutionContext, ProcessCleanup};
