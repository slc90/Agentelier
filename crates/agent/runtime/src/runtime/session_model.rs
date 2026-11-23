use agentelier_conversation::{
    Conversation, ConversationFailure, RecordEvent, SessionId, SessionView,
};
use agentelier_llm_model::ModelInfo;
use agentelier_session_store::SessionStore;
use tracing::{error, info};

pub(super) struct Change {
    pub(super) id: u64,
    pub(super) session: SessionId,
    pub(super) slug: String,
}

impl Change {
    pub(super) async fn save(
        &self,
        conversation: &mut Conversation,
        models: &[ModelInfo],
        store: &SessionStore,
    ) -> Result<SessionView, ConversationFailure> {
        let id = self.id;
        let session = self.session;
        let Some(prepared) = conversation.prepare_model_change(session, &self.slug, models)? else {
            info!(operation_id = id, session_id = %session, "会话模型未变化");
            return conversation
                .current()
                .ok_or_else(|| ConversationFailure::new("当前会话已变化"));
        };
        let commit = store
            .client()
            .append(RecordEvent::SessionModelChanged {
                session,
                model: Box::new(prepared.model.clone()),
            })
            .await
            .map_err(|failure| ConversationFailure::new(format!("无法保存会话模型：{failure}")))?;
        if commit.session != session {
            return Err(ConversationFailure::new("模型保存回执的会话不一致"));
        }
        if let Some(failure) = commit.summary_failure {
            error!(operation_id = id, session_id = %session, error = %failure, "模型已保存，摘要更新失败，可从记录重建");
        }
        let view = conversation.install_model_change(session, prepared)?;
        info!(operation_id = id, session_id = %session, model = view.settings.slug, sequence = commit.sequence, "会话模型已持久保存");
        Ok(view)
    }
}
