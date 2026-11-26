#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod ecs_access;
mod host;

pub use ecs_access::scenes::mount_native_scene;

pub use agentelier_bridge_connection::{
    ImageAttachmentAction, ImageAttachmentReply, MemoryDetail, MemoryInspection, MemoryReply,
    MemoryRequest, MemorySource, MemoryWorkState,
};
pub use agentelier_bridge_connection::{SkillEntry, SkillRequest, SkillSnapshot, UserSkillId};

pub use host::{
    AccountEvent, AgentHost, AgentHostPlugin, ConversationEvent, GlobalTaskNotificationEvent,
    GlobalTasksEvent, HostActivity, HostFailed, ImageAttachmentEvent, McpConfigEvent,
    McpMaterialEvent, McpStatusEvent, MemoryEvent, MemoryWorkEvent, ModelEvent, ReceiveAgentEvents,
    SavedAudioEvent, SkillsEvent,
};

pub use agentelier_bridge_connection::{ConfigChange, Server, ServerId, ServerSettings, Transport};

pub use agentelier_bridge_connection::{ConnectionState, ConnectionStatus};

pub use agentelier_bridge_connection::{
    MAX_MATERIAL_BYTES, MaterialKind, MaterialOrigin, MaterialPreview, MaterialReply,
    MaterialRequest, PromptChoice, ResourceChoice, expand_resource_uri,
};
