#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod generation;
mod model;
mod stream;

pub use generation::{GenerationRequest, generate};
pub use stream::{GenerationResult, OutputMessage, OutputText, StreamEvent};

pub use model::{
    ModelFailure, ModelFailureKind, ModelInfo, ModelReply, ModelRequest, ModelResult, execute,
};
