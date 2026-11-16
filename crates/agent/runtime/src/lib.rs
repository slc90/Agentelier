#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod operations;
mod runtime;
pub use operations::OperationSequence;

pub use runtime::{AgentEvent, AgentEventKind, AgentRuntime, DeliveryFailure, RuntimeFailure};
