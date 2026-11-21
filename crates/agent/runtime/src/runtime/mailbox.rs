use agentelier_conversation::{
    AgentId, AgentState, Conversation, ConversationFailure, MailboxEvent, RecordEvent, SessionId,
};
use agentelier_session_store::SessionStoreClient;
use std::collections::HashMap;
use tokio::sync::watch;

#[derive(Default)]
pub(super) struct MailboxActivity(HashMap<AgentId, watch::Sender<u64>>);

impl MailboxActivity {
    pub(super) fn subscribe(&mut self, target: AgentId, revision: u64) -> watch::Receiver<u64> {
        self.0
            .entry(target)
            .or_insert_with(|| watch::Sender::new(revision))
            .subscribe()
    }

    pub(super) async fn commit(
        &mut self,
        conversation: &mut Conversation,
        store: &SessionStoreClient,
        session: SessionId,
        event: MailboxEvent,
    ) -> Result<u64, ConversationFailure> {
        if !conversation.is_current(session) {
            return Err(ConversationFailure::new("Mailbox 不属于当前会话"));
        }
        let receiver = event.receiver();
        if let MailboxEvent::Queued { message } = &event {
            let check = |id| {
                let node = conversation
                    .agent_registry()
                    .get(session, id)
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                if node.state == AgentState::Closed {
                    return Err(ConversationFailure::new("消息来源或接收方已关闭"));
                }
                Ok(())
            };
            check(message.sender)?;
            check(receiver)?;
        }
        if let Some(sequence) = conversation.mailboxes().validate(&event)? {
            return Ok(sequence);
        }
        let receipt = store
            .append(RecordEvent::Mailbox {
                session,
                event: event.clone(),
            })
            .await;
        let sequence = match receipt {
            Ok(commit) if commit.session == session => commit.sequence,
            result => {
                let message = result.err().map_or_else(
                    || "Mailbox 提交回执属于其他会话".into(),
                    |failure| failure.to_string(),
                );
                conversation.fail_storage(message.clone());
                return Err(ConversationFailure::new(message));
            }
        };
        let (message_id, change) = match &event {
            MailboxEvent::Queued { message } => (message.id, "queued"),
            MailboxEvent::Claimed { claim } => (claim.message, "claimed"),
            MailboxEvent::Consumed { claim } => (claim.message, "consumed"),
        };
        conversation.mailboxes_mut().apply(event, sequence)?;
        if let Some(watch) = self.0.get(&receiver) {
            watch.send_replace(sequence);
        }
        tracing::debug!(session_id = %session, agent_id = %receiver, message_id = %message_id, sequence, change, "Mailbox 状态已持久提交");
        Ok(sequence)
    }
}
