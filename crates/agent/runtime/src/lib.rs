#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod operations;
mod runtime;
pub use agentelier_mcp::{
    ConfigChange, ConfigSnapshot, Server, ServerId, ServerSettings, Transport,
};
pub use operations::OperationSequence;

pub use runtime::{AgentEvent, AgentEventKind, AgentRuntime, DeliveryFailure, RuntimeFailure};

pub use agentelier_mcp::{ConnectionState, ConnectionStatus};

pub use agentelier_mcp::{
    MAX_MATERIAL_BYTES, MaterialOrigin, MaterialPreview, MaterialReply, MaterialRequest,
    ResourceChoice, expand_resource_uri,
};
