#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod discovery;
mod snapshot;

pub use discovery::{MAX_PROJECT_SOURCE_BYTES, ProjectContextFailure, collect};
pub use snapshot::{ProjectSnapshot, RuleSource};
