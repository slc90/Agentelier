#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod data_directory;
mod path_display;

pub use data_directory::{DataEnvironment, validate_data_directory};
pub use path_display::display_path;
