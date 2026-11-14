#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod credentials;
mod runtime;

pub use agentelier_login::{
    AuthenticatedRequest, SessionActivity, SessionFailure, SessionFailureKind,
};
pub use agentelier_login::{CredentialState, RestoredAccount};
pub use agentelier_login::{RemoteRevocation, SignOutFailure, SignOutStage};
pub use credentials::{CredentialClient, CredentialRequest};
pub use runtime::{
    AgentEvent, AgentEventKind, AgentRuntime, DeliveryFailure, OperationFailure, RuntimeFailure,
};
