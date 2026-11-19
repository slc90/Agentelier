#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod compaction;
pub use compaction::{CompactionItem, CompactionMetadata};
mod context;
mod generation;
mod memory;
pub use context::{ContextLimits, ResponseUsage};
mod model;
mod project;
mod protocol;
pub use project::{ProjectItem, ProjectPublication, PublicationKind};
mod stream;

pub use generation::{
    GenerationRequest, MAX_REQUEST_BYTES, PreparedGeneration, RequestCompaction, RequestHistory,
    RequestProjection, generate, prepare,
};
pub use memory::{MAX_MEMORY_SUMMARY_BYTES, MemoryMaterial, MemorySnapshot};
pub use protocol::{
    ArchivedPayload, HistoryAppend, ProtocolArchive, ProtocolHistory, ProtocolItemSummary,
    ProtocolOutput,
};
pub use stream::{FunctionCall, GenerationResult, OutputMessage, OutputText, StreamEvent};

pub use model::{
    BackgroundText, ModelFailure, ModelFailureKind, ModelInfo, ModelReply, ModelRequest,
    ModelResult, background_text, execute, task_model,
};

pub use protocol::{estimated_text_tokens, result_token_budget, result_tokens, result_wire_bytes};
