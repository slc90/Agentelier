#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod data_directory;
mod directory_settings;

pub use data_directory::{DataDirectoryReady, LoginUiHost, LoginUiPlugin};
