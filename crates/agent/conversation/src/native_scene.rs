use crate::{ConversationFailure, SessionId, ToolView};
use agentelier_tools::{
    PUBLISH_SCENE_TOOL, ToolStatus, UPDATE_SCENE_TOOL,
    ecs_access::{EcsError, SceneDefinition, SceneUpdate},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishedScene {
    pub id: SessionId,
    #[serde(default = "first_revision")]
    pub revision: u64,
    pub definition: Box<SceneDefinition>,
}

fn first_revision() -> u64 {
    1
}

impl PublishedScene {
    pub fn new(id: SessionId, definition: Box<SceneDefinition>) -> Self {
        Self {
            id,
            revision: first_revision(),
            definition,
        }
    }

    pub(crate) fn validate(
        &self,
        tool: &ToolView,
        status: ToolStatus,
    ) -> Result<(), ConversationFailure> {
        if self.id.is_nil()
            || self.revision != first_revision()
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

    pub(crate) fn next_revision(&self, update: &SceneUpdate) -> Result<u64, EcsError> {
        if self.id != update.id || self.revision != update.expected_revision {
            return Err(EcsError::new(
                "scene_revision_conflict",
                format!("Scene 当前修订为 {}", self.revision),
            ));
        }
        update.definition.validate()?;
        self.revision
            .checked_add(1)
            .ok_or_else(|| EcsError::new("scene_revision_limit", "Scene 修订序号已耗尽"))
    }

    pub(crate) fn apply(&mut self, update: SceneUpdate) -> Result<(), EcsError> {
        self.revision = self.next_revision(&update)?;
        self.definition = update.definition;
        Ok(())
    }
}

pub(crate) fn validate_update_source(
    tool: &ToolView,
    status: ToolStatus,
) -> Result<(), ConversationFailure> {
    if tool.name != UPDATE_SCENE_TOOL || tool.collaboration || status != ToolStatus::Succeeded {
        return Err(ConversationFailure::new("Scene 更新与来源工具结果不匹配"));
    }
    Ok(())
}
