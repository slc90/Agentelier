#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod credentials;
mod operations;
mod source;

pub use agentelier_login::{
    AuthenticatedRequest, CODEX_ORIGINATOR, CodexIdentity, CodexOAuthCredentials,
    CredentialVersion, DataEnvironment, SessionFailure, UsableCredentials,
};
pub use credentials::{CredentialClient, CredentialIntent, CredentialRequest, ExecutionIdentity};
pub use operations::{
    Account, AccountCommand, AccountEffect, AccountEvent, AccountEventKind, AccountFailure,
    DeliveryFailure, LoginPrompt, RefreshAttempt, RefreshCompletion,
};
pub use source::AccountSource;
