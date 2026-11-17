use super::ModelControl;
use agentelier_conversation::{
    Conversation, ConversationEvent, ConversationFailure, ConversationRequest, PreparedSession,
};
use agentelier_session_store::{Commit, SessionStore};
use std::io;
use tokio::sync::oneshot;
use tracing::{error, info};

pub(super) struct Creation {
    pub(super) id: u64,
    pub(super) control: ModelControl,
    prepared: PreparedSession,
    reply: oneshot::Receiver<io::Result<Commit>>,
}

impl Creation {
    pub(super) fn start(
        id: u64,
        control: ModelControl,
        request: ConversationRequest,
        conversation: &Conversation,
        store: &SessionStore,
    ) -> Result<Self, ConversationFailure> {
        let prepared = match request {
            ConversationRequest::Create(settings) => conversation.prepare(settings)?,
            ConversationRequest::Reset { session } => conversation.prepare_reset(session)?,
            _ => return Err(ConversationFailure::new("存档创建收到非创建请求")),
        };
        let reply = store
            .create(prepared.initial.clone())
            .map_err(|failure| ConversationFailure::new(format!("无法创建会话存档：{failure}")))?;
        info!(operation_id = id, session_id = %prepared.initial.session, "开始创建会话存档");
        Ok(Self {
            id,
            control,
            prepared,
            reply,
        })
    }

    pub(super) async fn wait(&mut self) -> io::Result<Commit> {
        (&mut self.reply)
            .await
            .map_err(|_| io::Error::other("会话存储工作者未返回提交结果"))?
    }

    pub(super) fn complete(
        self,
        result: io::Result<Commit>,
        conversation: &mut Conversation,
    ) -> ConversationEvent {
        self.control.finished.cancel();
        let commit = match result {
            Ok(commit) => commit,
            Err(failure) => {
                return ConversationEvent::Rejected(format!("无法创建会话存档：{failure}"));
            }
        };
        if commit.session != self.prepared.initial.session {
            return ConversationEvent::Rejected("会话存档提交身份不匹配".into());
        }
        if let Some(failure) = commit.summary_failure {
            error!(session_id = %commit.session, error = %failure, "会话初始记录已保存，摘要更新失败，可从记录重建");
        }
        let view = conversation.install(self.prepared);
        info!(operation_id = self.id, session_id = %view.id, sequence = commit.sequence, "会话初始存档已提交并打开");
        ConversationEvent::Created(view)
    }
}
