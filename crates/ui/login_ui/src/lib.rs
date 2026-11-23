#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod account;
mod authentication;
mod data_directory;
mod directory_settings;

pub use account::{AccountState, open_account_window};
pub use agentelier_account_bridge::QuotaWindow;
pub use data_directory::{DataDirectoryReady, LoginUiHost, LoginUiPlugin};
pub use directory_settings::{HomeContent, open_directory_settings};
