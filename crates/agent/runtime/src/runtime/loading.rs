use super::ModelControl;
use agentelier_conversation::{Conversation, ConversationEvent, RestoredSession, SessionId};
use agentelier_session_store::SessionStore;
use std::io;
use tokio::sync::oneshot;

pub(super) struct Loading {
    pub(super) id: u64,
    pub(super) control: Option<ModelControl>,
    session: SessionId,
    reply: oneshot::Receiver<io::Result<RestoredSession>>,
}

impl Loading {
    pub(super) fn start(
        id: u64,
        control: Option<ModelControl>,
        session: SessionId,
        store: &SessionStore,
    ) -> io::Result<Self> {
        Ok(Self {
            id,
            control,
            session,
            reply: store.client().load(session)?,
        })
    }

    pub(super) async fn wait(&mut self) -> io::Result<RestoredSession> {
        (&mut self.reply)
            .await
            .map_err(|_| io::Error::other("本地会话读取未返回结果"))?
    }

    pub(super) fn complete(
        self,
        result: io::Result<RestoredSession>,
        subject: Option<&str>,
        operations: &crate::OperationSequence,
        conversation: &mut Conversation,
    ) -> ConversationEvent {
        if let Some(control) = self.control {
            control.finished.cancel();
        }
        let result = result.and_then(|mut restored| {
            if restored.state.session != self.session {
                return Err(io::Error::other("读取的会话身份不匹配"));
            }
            for input in &mut restored.state.queue {
                input.operation_id = operations.allocate()?;
            }
            Ok(restored)
        });
        match result {
            Ok(restored) => {
                let view = conversation.install_restored(restored, subject);
                tracing::info!(session_id = %view.id, "本地会话已打开，未启动模型或待办");
                ConversationEvent::Opened(view)
            }
            Err(failure) => {
                ConversationEvent::ArchiveFailed(format!("无法打开本地会话：{failure}"))
            }
        }
    }
}
