#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod host;

pub use host::{
    AccountEvent, AgentHost, AgentHostPlugin, HostFailed, ModelEvent, ReceiveAgentEvents,
};
