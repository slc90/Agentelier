#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod host;

pub use agentelier_bridge_connection::{
    ImageAttachmentAction, ImageAttachmentReply, MemoryDetail, MemoryInspection, MemoryReply,
    MemoryRequest, MemorySource,
};

pub use host::{
    AccountEvent, AgentHost, AgentHostPlugin, ConversationEvent, HostActivity, HostFailed,
    ImageAttachmentEvent, McpConfigEvent, McpMaterialEvent, McpStatusEvent, MemoryEvent,
    ModelEvent, ReceiveAgentEvents,
};

pub use agentelier_bridge_connection::{ConfigChange, Server, ServerId, ServerSettings, Transport};

pub use agentelier_bridge_connection::SourceChange;
pub use agentelier_bridge_connection::{ConnectionState, ConnectionStatus};

pub use agentelier_bridge_connection::{
    MAX_MATERIAL_BYTES, MaterialKind, MaterialOrigin, MaterialPreview, MaterialReply,
    MaterialRequest, PromptChoice, ResourceChoice, expand_resource_uri,
};
