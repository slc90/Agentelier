use agentelier_conversation::{ConversationEvent, SessionId};
use agentelier_session_store::{RenameCommit, SessionStore};
use std::io;
use tokio::sync::oneshot;

pub(super) struct Renaming {
    pub(super) id: u64,
    session: SessionId,
    reply: oneshot::Receiver<io::Result<RenameCommit>>,
}
impl Renaming {
    pub(super) fn start(
        id: u64,
        session: SessionId,
        title: String,
        store: &SessionStore,
    ) -> io::Result<Self> {
        Ok(Self {
            id,
            session,
            reply: store.client().rename(session, title)?,
        })
    }
    pub(super) async fn wait(&mut self) -> io::Result<RenameCommit> {
        (&mut self.reply)
            .await
            .map_err(|_| io::Error::other("会话名称提交未返回结果"))?
    }
    pub(super) fn complete(self, result: io::Result<RenameCommit>) -> ConversationEvent {
        let result = result.and_then(|commit| {
            if commit.summary.session != self.session { return Err(io::Error::other("名称回执的会话身份不一致")); }
            if let Some(failure) = &commit.summary_failure { tracing::error!(session_id = %self.session, error = %failure, "会话名称已提交，摘要更新失败，可从权威记录重建"); }
            tracing::info!(operation_id = self.id, session_id = %self.session, "会话名称已提交");
            Ok(agentelier_conversation::ArchiveEntry { summary: commit.summary, error: commit.summary_failure.map(|failure| format!("名称已保存，但摘要更新失败：{failure}")) })
        }).map_err(|failure| failure.to_string());
        ConversationEvent::Renamed {
            session: self.session,
            result,
        }
    }
}
