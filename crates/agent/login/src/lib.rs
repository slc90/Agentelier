#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod authentication;
mod browser;
mod codex;
mod credential_commit;
mod data_directory;
mod session;
mod sign_out;

pub use authentication::{
    AccountSummary, AuthFile, CredentialState, RestoreFailure, RestoredAccount,
};
pub use browser::{AuthorizationContext, BrowserLogin, LoginFailure, LoginStage, VerifiedLogin};
pub use codex::{
    CodexAccountSummary, CodexCredentials, CodexIdentity, CodexReadFailure, read_codex_credentials,
};
pub use data_directory::{DataEnvironment, validate_data_directory};
pub use session::{
    AuthenticatedRequest, CredentialPreparation, CredentialVersion, SessionActivity,
    SessionFailure, SessionFailureKind, UsableCredentials,
};
pub use sign_out::{RemoteRevocation, SignOutFailure, SignOutPreparation, SignOutStage};
