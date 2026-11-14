#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod model;

pub use model::{ModelEventKind, ModelFailure, ModelFailureKind, ModelReply, convert};
