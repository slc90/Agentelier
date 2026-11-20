#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod credentials;
mod operations;
mod source;

pub use agentelier_login::{
    AuthenticatedRequest, CodexIdentity, CredentialState, RemoteRevocation, RestoredAccount,
    SessionActivity, SessionFailure, SessionFailureKind, SignOutStage, UsableCredentials,
};
pub use credentials::{CredentialClient, CredentialIntent, CredentialRequest, ExecutionIdentity};
pub use operations::{
    Account, AccountCommand, AccountEvent, AccountEventKind, AccountFailure, DeliveryFailure,
    OperationFailure,
};
pub use source::{AccountSource, SourceChange, SourceSettings};
