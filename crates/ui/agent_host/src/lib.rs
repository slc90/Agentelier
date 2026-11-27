#[cfg(not(all(target_os = "windows", target_pointer_width = "64")))]
compile_error!("This application supports Windows x64 only");

mod ecs_access;
mod host;
mod scene_state;

pub use agentelier_bridge_connection::ecs_access::{
    EcsError, SceneDefinition, SceneStored, SceneTarget,
};
pub use agentelier_bridge_connection::widget;
pub use agentelier_conversation_bridge::SceneState;
pub use ecs_access::scenes::{
    MountedScene, WidgetLiveState, mount_native_scene, mount_scene_child, patch_native_scene,
    prepare_scene_definition, replace_scene_children, restore_scene_definition,
    take_widget_candidate, validating_scene,
};
pub use scene_state::{SceneSaveError, SceneStateWrites, flush_scene_state};

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

pub use agentelier_bridge_connection::{
    ComposerAssetFuture, ComposerAssetOwner, ComposerAssetReply, ComposerAssetRequest,
    ComposerImageSource,
};
