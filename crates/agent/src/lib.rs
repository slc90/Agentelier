#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod runtime;

pub use agentelier_login::CredentialState;
pub use runtime::{
    AgentEvent, AgentEventKind, AgentRuntime, DeliveryFailure, OperationFailure, RuntimeFailure,
};
