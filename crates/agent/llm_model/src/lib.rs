#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod compaction;
pub use compaction::{CompactionItem, CompactionMetadata};
mod context;
mod generation;
pub use context::{ContextLimits, ResponseUsage};
mod model;
mod project;
mod protocol;
pub use project::{ProjectItem, ProjectPublication, PublicationKind};
mod stream;

pub use generation::{GenerationRequest, PreparedGeneration, RequestProjection, generate, prepare};
pub use protocol::{
    ArchivedPayload, ProtocolArchive, ProtocolHistory, ProtocolItemSummary, ProtocolOutput,
};
pub use stream::{FunctionCall, GenerationResult, OutputMessage, OutputText, StreamEvent};

pub use model::{
    BackgroundText, ModelFailure, ModelFailureKind, ModelInfo, ModelReply, ModelRequest,
    ModelResult, background_text, execute, task_model,
};

pub use protocol::estimated_text_tokens;
