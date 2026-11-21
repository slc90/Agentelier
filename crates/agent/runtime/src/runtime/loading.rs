use super::ModelControl;
use agentelier_conversation::{
    AgentId, Conversation, ConversationEvent, RestoredSession, SessionId,
};
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
        operations: &crate::OperationSequence,
        conversation: &mut Conversation,
        mcp: &agentelier_mcp::Connections,
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
                let tools = mcp.restore_registrations(restored.tool_definitions());
                let agent = AgentId::for_root(self.session);
                match conversation.install_restored(restored, &tools) {
                    Ok((view, instance)) => {
                        tracing::info!(operation_id = self.id, session_id = %view.id, agent_id = %agent, agent_path = "root", runtime_instance = %instance, model = %view.settings.slug, effort = ?view.settings.effort, "本地会话已打开并登记 Root Agent");
                        ConversationEvent::Opened(view)
                    }
                    Err(failure) => {
                        tracing::error!(operation_id = self.id, session_id = %self.session, agent_id = %agent, agent_path = "root", error = %failure, "打开本地会话时 Root Agent 登记失败");
                        ConversationEvent::ArchiveFailed(format!("无法登记 Root Agent：{failure}"))
                    }
                }
            }
            Err(failure) => {
                ConversationEvent::ArchiveFailed(format!("无法打开本地会话：{failure}"))
            }
        }
    }
}
