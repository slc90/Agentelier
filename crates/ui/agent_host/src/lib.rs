#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod host;

pub use host::{
    AccountEvent, AgentHost, AgentHostPlugin, ConversationEvent, HostActivity, HostFailed,
    McpConfigEvent, ModelEvent, ReceiveAgentEvents,
};

pub use agentelier_bridge_connection::{ConfigChange, Server, ServerId, ServerSettings, Transport};
