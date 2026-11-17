#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod credentials;
mod operations;

pub use agentelier_login::{
    AuthenticatedRequest, CredentialState, RemoteRevocation, RestoredAccount, SessionActivity,
    SessionFailure, SessionFailureKind, SignOutStage, UsableCredentials,
};
pub use credentials::{CredentialClient, CredentialIntent, CredentialRequest, ExecutionIdentity};
pub use operations::{
    Account, AccountCommand, AccountEvent, AccountEventKind, AccountFailure, DeliveryFailure,
    OperationFailure,
};
