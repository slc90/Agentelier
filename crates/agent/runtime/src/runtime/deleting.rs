use super::ModelControl;
use agentelier_conversation::{Conversation, ConversationEvent, SessionId};
use agentelier_session_store::{DeleteCommit, SessionStore};
use std::io;
use tokio::sync::oneshot;

pub(super) struct Deleting {
    pub(super) id: u64,
    control: ModelControl,
    session: SessionId,
    reply: oneshot::Receiver<io::Result<DeleteCommit>>,
}
impl Deleting {
    pub(super) fn session(&self) -> SessionId {
        self.session
    }

    pub(super) fn start(
        id: u64,
        control: ModelControl,
        session: SessionId,
        store: &SessionStore,
    ) -> io::Result<Self> {
        Ok(Self {
            id,
            control,
            session,
            reply: store.client().delete(session)?,
        })
    }
    pub(super) async fn wait(&mut self) -> io::Result<DeleteCommit> {
        (&mut self.reply)
            .await
            .map_err(|_| io::Error::other("会话删除未返回结果"))?
    }
    pub(super) fn complete(
        self,
        result: io::Result<DeleteCommit>,
        conversation: &mut Conversation,
    ) -> ConversationEvent {
        self.control.finished.cancel();
        match result {
            Ok(commit) if commit.session == self.session => {
                let was_open = conversation
                    .current()
                    .is_some_and(|view| view.id == self.session);
                conversation.remove(self.session);
                let cleanup_error = commit.cleanup_error.map(|failure| failure.to_string());
                tracing::info!(operation_id = self.id, session_id = %self.session, cleanup_pending = cleanup_error.is_some(), "会话删除状态已提交");
                ConversationEvent::Deleted {
                    session: self.session,
                    cleanup_error,
                    was_open,
                }
            }
            Ok(_) => ConversationEvent::Rejected("删除回执的会话身份不匹配".into()),
            Err(failure) => ConversationEvent::Rejected(format!("会话未删除：{failure}")),
        }
    }
}
