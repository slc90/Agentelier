#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod account;
mod data_directory;
mod directory_settings;

pub use account::{
    AccountPhase, AccountState, AccountSummary, CredentialState, LoginAction, RestoreAccount,
    SessionActivity,
};
pub use data_directory::{DataDirectoryReady, LoginUiHost, LoginUiPlugin};
