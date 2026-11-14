#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod authentication;
mod browser;
mod credential_commit;
mod data_directory;
mod session;

pub use authentication::{
    AccountSummary, AuthFile, CredentialState, RestoreFailure, RestoredAccount,
};
pub use browser::{AuthorizationContext, BrowserLogin, LoginFailure, LoginStage, VerifiedLogin};
pub use data_directory::{DataEnvironment, validate_data_directory};
pub use session::{
    AuthenticatedRequest, CredentialPreparation, CredentialVersion, SessionActivity,
    SessionFailure, SessionFailureKind, UsableCredentials,
};
