#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod connections;
mod discovery;
mod settings;
mod stdio;

pub use connections::{ConnectionState, ConnectionStatus, Connections, ServerCapabilities};

pub use settings::{
    ConfigChange, ConfigSnapshot, ConfigStore, Server, ServerId, ServerSettings, Transport,
};
