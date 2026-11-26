use crate::{ConversationFailure, SessionId, ToolView};
use agentelier_tools::{PUBLISH_SCENE_TOOL, ToolStatus, ecs_access::SceneDefinition};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishedScene {
    pub id: SessionId,
    pub definition: Box<SceneDefinition>,
}

impl PublishedScene {
    pub(crate) fn validate(
        &self,
        tool: &ToolView,
        status: ToolStatus,
    ) -> Result<(), ConversationFailure> {
        if self.id.is_nil()
            || tool.name != PUBLISH_SCENE_TOOL
            || tool.collaboration
            || status != ToolStatus::Succeeded
        {
            return Err(ConversationFailure::new("Scene 产物与来源工具结果不匹配"));
        }
        self.definition
            .validate()
            .map_err(|error| ConversationFailure::new(error.to_string()))
    }
}
