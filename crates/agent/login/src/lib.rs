#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod codex;
mod data_directory;
mod oauth;
mod session;
pub use codex::{CodexAccountSummary, CodexIdentity, CodexReadFailure};
pub use data_directory::{DataEnvironment, validate_data_directory};
pub use oauth::{
    BrowserLogin, CODEX_ORIGINATOR, CodexOAuthCredentials, CredentialStore, CredentialStoreFailure,
    OAuthFailure, RefreshFailure, RevocationFailure,
};
pub use session::{AuthenticatedRequest, CredentialVersion, SessionFailure, UsableCredentials};
