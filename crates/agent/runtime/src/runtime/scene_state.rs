use agentelier_conversation::{Conversation, ConversationFailure, SceneState};
use agentelier_session_store::SessionStoreClient;
use agentelier_tools::ecs_access::SceneTarget;

pub(super) async fn save(
    conversation: &mut Conversation,
    store: &SessionStoreClient,
    target: SceneTarget,
    state: SceneState,
) -> Result<Option<u64>, ConversationFailure> {
    let Some(record) = conversation.scene_state_record(target, &state)? else {
        return Ok(None);
    };
    let commit = store
        .append(record)
        .await
        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
    if commit.session != target.session {
        return Err(ConversationFailure::new("Scene 状态回执会话不匹配"));
    }
    conversation.confirm_scene_state(target, state)?;
    tracing::debug!(
        session_id = %target.session,
        artifact_id = %target.artifact,
        sequence = commit.sequence,
        "Scene 用户状态已提交"
    );
    Ok(Some(commit.sequence))
}
