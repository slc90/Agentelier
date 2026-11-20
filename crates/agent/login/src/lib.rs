#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod codex;
mod data_directory;
mod session;
pub use codex::{
    CodexAccountSummary, CodexCredentials, CodexIdentity, CodexReadFailure, read_codex_credentials,
};
pub use data_directory::{DataEnvironment, validate_data_directory};
pub use session::{AuthenticatedRequest, CredentialVersion, SessionFailure, UsableCredentials};
