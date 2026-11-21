use agentelier_conversation::{
    AgentId, AgentState, Conversation, ConversationFailure, MailboxEvent, RecordEvent, SessionId,
};
use agentelier_session_store::SessionStoreClient;
use agentelier_tools::{
    ProcessOrigin, ToolDefinition, ToolMode, ToolRegistration, ToolResult, ToolSource, ToolStatus,
};
use agentelier_turn_runner::InboxRequest;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot, watch};

pub(super) enum Command {
    Send(Outgoing),
    Inbox(InboxRequest),
}

pub(super) struct Outgoing {
    pub(super) origin: ProcessOrigin,
    pub(super) target: String,
    pub(super) message: String,
    pub(super) cancel: tokio_util::sync::CancellationToken,
    pub(super) reply: oneshot::Sender<ToolResult>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    target: String,
    message: String,
}

pub(super) fn registration(commands: mpsc::Sender<Command>) -> ToolRegistration {
    const NAME: &str = "send_message";
    let definition = ToolDefinition {
        name: NAME.into(),
        description: "Queue a message for another Agent in the current session. Use its stable Agent ID or exact path. The receipt confirms durable delivery to the mailbox. The target reads it at a safe request boundary. This tool does not start an idle Agent. Self-messages are rejected.".into(),
        parameters: json!({"type":"object","properties":{
            "target":{"type":"string"},"message":{"type":"string","maxLength":agentelier_conversation::MAX_AGENT_MESSAGE_CHARS}
        },"required":["target","message"],"additionalProperties":false}),
        source: ToolSource::Extension { provider: "agentelier.collaboration".into(), tool: NAME.into() },
        strict: true,
        mode: ToolMode::Synchronous,
        display_source: None,
        output_schema: None,
    };
    ToolRegistration::with_origin(definition, move |_, raw, cancel, origin| {
        let commands = commands.clone();
        Box::pin(async move {
            let arguments: Arguments = match serde_json::from_str(raw) {
                Ok(args) => args,
                Err(failure) => return ToolResult::not_executed("invalid_message", &failure.to_string()),
            };
            let (reply, receipt) = oneshot::channel();
            let command = Command::Send(Outgoing { origin, target: arguments.target, message: arguments.message.trim().into(), cancel: cancel.clone(), reply });
            tokio::select! { biased;
                _ = cancel.cancelled() => return ToolResult::not_executed("message_cancelled", "消息在接纳前取消"),
                result = commands.send(command) => if result.is_err() {
                    return ToolResult::not_executed("mailbox_closed", "Mailbox 已停止接纳");
                }
            }
            tokio::select! { biased;
                result = receipt => result.unwrap_or_else(|_| ToolResult::error(ToolStatus::Unconfirmed, "message_unconfirmed", "Mailbox 未返回提交回执")),
                _ = cancel.cancelled() => ToolResult::error(ToolStatus::Unconfirmed, "message_unconfirmed", "消息取消后仍需核对存档"),
            }
        })
    }).text_result()
}

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
