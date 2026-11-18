#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod host;

pub use host::{
    AccountEvent, AgentHost, AgentHostPlugin, ConversationEvent, HostActivity, HostFailed,
    McpConfigEvent, McpMaterialEvent, McpStatusEvent, ModelEvent, ReceiveAgentEvents,
};

pub use agentelier_bridge_connection::{ConfigChange, Server, ServerId, ServerSettings, Transport};

pub use agentelier_bridge_connection::{ConnectionState, ConnectionStatus};

pub use agentelier_bridge_connection::{
    MAX_MATERIAL_BYTES, MaterialOrigin, MaterialPreview, MaterialReply, MaterialRequest,
    ResourceChoice, expand_resource_uri,
};
