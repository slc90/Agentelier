use crate::{Conversation, ConversationFailure, RecordEvent, SessionId, ToolView};
use agentelier_tools::{
    PUBLISH_SCENE_TOOL, ToolStatus, UPDATE_SCENE_TOOL,
    ecs_access::{EcsError, SceneDefinition, SceneTarget, SceneUpdate},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishedScene {
    pub id: SessionId,
    #[serde(default = "first_revision")]
    pub revision: u64,
    pub definition: Box<SceneDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SceneState>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneState {
    pub revision: u64,
    pub values: serde_json::Map<String, serde_json::Value>,
}

pub struct SceneStateReceipt {
    pub target: SceneTarget,
    pub revision: u64,
    pub result: Result<Option<u64>, String>,
}

impl SceneState {
    pub fn validate(&self) -> Result<(), ConversationFailure> {
        if self.revision == 0
            || self.values.is_empty()
            || self.values.iter().any(|(name, value)| {
                name.trim().is_empty()
                    || name.len() > 64
                    || !(value.is_boolean() || value.is_number() || value.is_string())
            })
        {
            return Err(ConversationFailure::new("Scene 状态修订或参数值无效"));
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|error| ConversationFailure::new(error.to_string()))?;
        if bytes.len() > 512 * 1024 {
            return Err(ConversationFailure::new("Scene 状态超过 512 KiB"));
        }
        Ok(())
    }
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
            state: None,
        }
    }

    pub(crate) fn validate(
        &self,
        tool: &ToolView,
        status: ToolStatus,
    ) -> Result<(), ConversationFailure> {
        if self.id.is_nil()
            || self.revision != first_revision()
            || self.state.is_some()
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
        self.state = None;
        Ok(())
    }

    pub(crate) fn validate_state(&self, state: &SceneState) -> Result<(), ConversationFailure> {
        if state.revision != self.revision {
            return Err(ConversationFailure::new(format!(
                "Scene 的当前定义修订为 {}",
                self.revision
            )));
        }
        state.validate()
    }
}

impl Conversation {
    pub fn scene_state_record(
        &self,
        target: SceneTarget,
        state: &SceneState,
    ) -> Result<Option<RecordEvent>, ConversationFailure> {
        let session = self
            .session
            .as_ref()
            .filter(|session| {
                session.view.id == target.session
                    && session.view.settings.kind.is_user()
                    && session.view.settings.background.is_none()
            })
            .ok_or_else(|| ConversationFailure::new("Scene 状态不属于当前用户会话"))?;
        let scene = session
            .view
            .turn
            .iter()
            .map(Box::as_ref)
            .chain(&session.view.previous_turns)
            .flat_map(|turn| &turn.tools)
            .filter_map(|tool| tool.published_scene.as_ref())
            .find(|scene| scene.id == target.artifact)
            .ok_or_else(|| ConversationFailure::new("会话没有此 Scene 产物"))?;
        scene.validate_state(state)?;
        Ok(
            (scene.state.as_ref() != Some(state)).then(|| RecordEvent::SceneState {
                session: target.session,
                artifact: target.artifact,
                state: state.clone(),
            }),
        )
    }

    pub fn confirm_scene_state(
        &mut self,
        target: SceneTarget,
        state: SceneState,
    ) -> Result<(), ConversationFailure> {
        let session = self
            .session
            .as_mut()
            .filter(|session| session.view.id == target.session)
            .ok_or_else(|| ConversationFailure::new("Scene 状态回执不属于当前会话"))?;
        let scene = session
            .view
            .turn
            .iter_mut()
            .map(Box::as_mut)
            .chain(&mut session.view.previous_turns)
            .flat_map(|turn| &mut turn.tools)
            .filter_map(|tool| tool.published_scene.as_mut())
            .find(|scene| scene.id == target.artifact)
            .ok_or_else(|| ConversationFailure::new("Scene 状态回执缺少产物"))?;
        scene.validate_state(&state)?;
        scene.state = Some(state);
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
