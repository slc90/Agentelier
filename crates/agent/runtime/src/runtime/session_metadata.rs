use agentelier_conversation::{ConversationEvent, SessionId};
use agentelier_session_store::{MetadataCommit, SessionStore};
use std::io;
use tokio::sync::oneshot;

pub(super) struct SavingMetadata {
    pub(super) id: u64,
    session: SessionId,
    reply: oneshot::Receiver<io::Result<MetadataCommit>>,
}
impl SavingMetadata {
    pub(super) fn start(
        id: u64,
        session: SessionId,
        change: agentelier_conversation::SessionMetadataChange,
        store: &SessionStore,
    ) -> io::Result<Self> {
        Ok(Self {
            id,
            session,
            reply: store.client().change_metadata(session, change)?,
        })
    }
    pub(super) async fn wait(&mut self) -> io::Result<MetadataCommit> {
        (&mut self.reply)
            .await
            .map_err(|_| io::Error::other("会话元数据提交未返回结果"))?
    }
    pub(super) fn complete(self, result: io::Result<MetadataCommit>) -> ConversationEvent {
        let result = result.and_then(|commit| {
            if commit.summary.session != self.session { return Err(io::Error::other("元数据回执的会话身份不一致")); }
            if let Some(failure) = &commit.summary_failure { tracing::error!(session_id = %self.session, error = %failure, "会话元数据已提交，摘要更新失败，可从权威记录重建"); }
            tracing::info!(operation_id = self.id, session_id = %self.session, "会话元数据已提交");
            Ok(agentelier_conversation::ArchiveEntry { summary: commit.summary, error: commit.summary_failure.map(|failure| format!("元数据已保存，但摘要更新失败：{failure}")) })
        }).map_err(|failure| failure.to_string());
        ConversationEvent::MetadataSaved {
            session: self.session,
            result,
        }
    }
}
