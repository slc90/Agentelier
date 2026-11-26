#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod call;
mod connections;
mod discovery;
mod http;

mod materials;
mod settings;
mod stdio;

pub use connections::{ConnectionState, ConnectionStatus, Connections, ServerCapabilities};

pub use settings::{
    ConfigChange, ConfigSnapshot, ConfigStore, Server, ServerId, ServerSettings, Transport,
};

pub use materials::{
    MAX_MATERIAL_BYTES, MaterialKind, MaterialOrigin, MaterialPreview, MaterialReply,
    MaterialRequest, PromptChoice, ResourceChoice, expand_resource_uri,
};
