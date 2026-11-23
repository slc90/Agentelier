#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod account;
mod authentication;
mod data_directory;

pub use account::{AccountState, login_entry};
pub use agentelier_account_bridge::QuotaWindow;
pub use data_directory::{DataDirectoryReady, HomeContent, LoginUiHost, LoginUiPlugin};
