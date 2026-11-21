use crate::{AgentId, AgentIdentity, ConversationFailure, SessionId, TurnId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const MAX_AGENT_MESSAGE_CHARS: usize = 16_384;
const MAX_PENDING_PER_AGENT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentMessageId(SessionId);

impl AgentMessageId {
    pub fn allocate() -> Self {
        Self(SessionId::new_v4())
    }

    pub fn initial(identity: &AgentIdentity) -> Self {
        Self(identity.thread.session_id())
    }
}

impl std::fmt::Display for AgentMessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    QueueOnly,
    TriggerTurn,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: AgentMessageId,
    pub sender: AgentId,
    pub receiver: AgentId,
    pub turn: TurnId,
    pub call_id: String,
    pub mode: DeliveryMode,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageClaim {
    pub receiver: AgentId,
    pub message: AgentMessageId,
    pub turn: TurnId,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum MailboxEvent {
    Queued { message: AgentMessage },
    Claimed { claim: MessageClaim },
    Consumed { claim: MessageClaim },
}

impl MailboxEvent {
    pub fn receiver(&self) -> AgentId {
        match self {
            Self::Queued { message } => message.receiver,
            Self::Claimed { claim } | Self::Consumed { claim } => claim.receiver,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageStatus {
    Pending,
    Claimed(TurnId),
    Consumed(TurnId),
}

pub struct MailboxEntry {
    pub message: AgentMessage,
    pub queued_at: u64,
    pub status: MessageStatus,
}

#[derive(Default)]
pub struct Mailboxes {
    messages: HashMap<AgentMessageId, MailboxEntry>,
    last_sequence: u64,
    closed: bool,
}

impl Mailboxes {
    pub fn find(&self, id: AgentMessageId) -> Option<&MailboxEntry> {
        self.messages.get(&id)
    }

    pub fn oldest(&self, receiver: AgentId) -> Option<&MailboxEntry> {
        self.messages
            .values()
            .filter(|entry| {
                entry.message.receiver == receiver
                    && !matches!(entry.status, MessageStatus::Consumed(_))
            })
            .min_by_key(|entry| entry.queued_at)
    }

    pub fn revision(&self) -> u64 {
        self.last_sequence
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn validate(&self, event: &MailboxEvent) -> Result<Option<u64>, ConversationFailure> {
        match event {
            MailboxEvent::Queued { message } => {
                if let Some(saved) = self.find(message.id) {
                    if saved.message != *message {
                        return Err(ConversationFailure::new("消息 ID 已用于其他载荷"));
                    }
                    return Ok(Some(saved.queued_at));
                }
                if self.closed {
                    return Err(ConversationFailure::new("会话 Mailbox 正在关闭"));
                }
                if message.id.0.is_nil()
                    || message.turn.0 == 0
                    || message.call_id.trim().is_empty()
                    || message.text.trim().is_empty()
                    || message.text.chars().count() > MAX_AGENT_MESSAGE_CHARS
                {
                    return Err(ConversationFailure::new("消息缺少有效身份、来源或正文"));
                }
                let pending = self
                    .messages
                    .values()
                    .filter(|entry| {
                        entry.message.receiver == message.receiver
                            && !matches!(entry.status, MessageStatus::Consumed(_))
                    })
                    .count();
                if pending >= MAX_PENDING_PER_AGENT {
                    return Err(ConversationFailure::new("接收方 Mailbox 已满"));
                }
            }
            MailboxEvent::Claimed { claim } => {
                if claim.turn.0 == 0
                    || !self.oldest(claim.receiver).is_some_and(|entry| {
                        entry.message.id == claim.message && entry.status == MessageStatus::Pending
                    })
                {
                    return Err(ConversationFailure::new(
                        "消息领取与 Mailbox 当前顺序不一致",
                    ));
                }
            }
            MailboxEvent::Consumed { claim } => {
                if !self.find(claim.message).is_some_and(|entry| {
                    entry.message.receiver == claim.receiver
                        && entry.status == MessageStatus::Claimed(claim.turn)
                }) {
                    return Err(ConversationFailure::new(
                        "消息消费缺少同一接收方与 Turn 的领取记录",
                    ));
                }
            }
        }
        Ok(None)
    }

    pub fn apply(&mut self, event: MailboxEvent, sequence: u64) -> Result<(), ConversationFailure> {
        if sequence <= self.last_sequence || self.validate(&event)?.is_some() {
            return Err(ConversationFailure::new(
                "Mailbox 存档序号无效或重复接纳同一消息",
            ));
        }
        match event {
            MailboxEvent::Queued { message } => {
                self.messages.insert(
                    message.id,
                    MailboxEntry {
                        message,
                        queued_at: sequence,
                        status: MessageStatus::Pending,
                    },
                );
            }
            MailboxEvent::Claimed { claim } => {
                self.update(claim.message, MessageStatus::Claimed(claim.turn))?
            }
            MailboxEvent::Consumed { claim } => {
                self.update(claim.message, MessageStatus::Consumed(claim.turn))?
            }
        }
        self.last_sequence = sequence;
        Ok(())
    }

    fn update(
        &mut self,
        id: AgentMessageId,
        status: MessageStatus,
    ) -> Result<(), ConversationFailure> {
        let entry = self
            .messages
            .get_mut(&id)
            .ok_or_else(|| ConversationFailure::new("Mailbox 缺少目标准入记录"))?;
        entry.status = status;
        Ok(())
    }
}
