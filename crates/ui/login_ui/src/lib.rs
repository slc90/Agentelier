#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod account;
mod authentication;
mod data_directory;
mod directory_settings;
mod model;

pub use account::{
    AccountPhase, AccountState, AccountSummary, CredentialState, LoginAction, RestoreAccount,
    SessionActivity,
};
pub use data_directory::{DataDirectoryReady, LoginUiHost, LoginUiPlugin};
pub use model::{ModelState, VerifyModel};
