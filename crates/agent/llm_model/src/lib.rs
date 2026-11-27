#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod codex_socket;
mod compaction;
pub use compaction::{CompactionItem, CompactionMetadata};
mod context;
mod expression;
mod generation;
mod images;
mod memory;
pub use context::{ContextLimits, ResponseUsage};
mod model;
mod project;
mod protocol;
pub use project::{ProjectItem, ProjectPublication, PublicationKind};
mod stream;
pub use agentelier_account::CodexIdentity;
pub use images::generate_image;
mod transcript_material;
mod transcription;
pub use transcript_material::{TranscriptEvidence, TranscriptMaterial, TranscriptReference};
pub use transcription::{MAX_TRANSCRIPT_BYTES, transcribe_audio};

pub use expression::{
    ExpressionContext, ExpressionExcerpt, ExpressionMaterial, expression_excerpt,
};
pub use generation::{
    GenerationConnection, GenerationRequest, McpMaterial, PreparedGeneration, RecordedModelSource,
    RequestCompaction, RequestHistory, RequestProjection, generate, prepare,
};
pub use memory::{
    MAX_MEMORY_SUMMARY_BYTES, MEMORY_SUMMARY_PROMPT_BYTES, MemoryMaterial, MemoryReference,
    MemoryScope, MemorySnapshot,
};
pub use protocol::{
    ArchivedPayload, HistoryAppend, ProtocolArchive, ProtocolHistory, ProtocolItemSummary,
    ProtocolOutput,
};
pub use stream::{FunctionCall, GenerationResult, OutputMessage, OutputText, StreamEvent};

pub use model::{
    AgentModelError, BackgroundText, ModelFailure, ModelFailureKind, ModelInfo, ModelReply,
    ModelRequest, ModelResult, MultiAgentCapability, background_text, codex_tool_contract, execute,
    select_agent_model, select_session_model, task_catalog, task_model,
};

pub use protocol::{
    UserImage, UserMessage, estimated_text_tokens, result_token_budget, result_tokens,
};
