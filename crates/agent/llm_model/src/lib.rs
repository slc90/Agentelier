#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod generation;
mod model;
mod protocol;
mod stream;

pub use generation::{GenerationRequest, generate};
pub use protocol::{
    ArchivedPayload, ProtocolArchive, ProtocolHistory, ProtocolItemSummary, ProtocolOutput,
};
pub use stream::{FunctionCall, GenerationResult, OutputMessage, OutputText, StreamEvent};

pub use model::{
    ModelFailure, ModelFailureKind, ModelInfo, ModelReply, ModelRequest, ModelResult, execute,
};
