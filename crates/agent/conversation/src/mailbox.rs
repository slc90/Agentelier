use crate::{
    AgentId, AgentIdentity, AgentPath, AgentState, ConversationFailure, SessionId, TurnId,
};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<TaskCompletion>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCompletion {
    pub task: AgentMessageId,
    pub assigned_by: AgentId,
    pub path: AgentPath,
    pub agent_turn: TurnId,
    pub state: AgentState,
    pub assignment: String,
}

impl AgentMessage {
    pub fn render(&self) -> String {
        if let Some(completion) = &self.completion {
            return format!(
                "Task result from {} (Agent {}, Turn {}, task {} assigned by Agent {} in Turn {}, call {}). Status: {}.\nAssignment:\n{}\nResult:\n{}",
                completion.path,
                self.sender,
                completion.agent_turn.0,
                completion.task,
                completion.assigned_by,
                self.turn.0,
                self.call_id,
                completion.state.name(),
                completion.assignment,
                self.text
            );
        }
        format!(
            "Message from collaborating Agent {} (message {}, source Turn {}):\n{}",
            self.sender, self.id, self.turn.0, self.text
        )
    }
}

pub struct ClaimedMessage {
    pub claim: MessageClaim,
    pub message: AgentMessage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageClaim {
    pub receiver: AgentId,
    pub message: AgentMessageId,
    pub turn: TurnId,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wake: bool,
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

    pub fn find_source(
        &self,
        sender: AgentId,
        turn: TurnId,
        call_id: &str,
    ) -> Option<&MailboxEntry> {
        self.messages.values().find(|entry| {
            entry.message.sender == sender
                && entry.message.turn == turn
                && entry.message.call_id == call_id
        })
    }

    pub fn next_for_turn(&self, receiver: AgentId, turn: TurnId) -> Option<&MailboxEntry> {
        self.next_input(receiver, turn, false)
    }

    fn next_input(&self, receiver: AgentId, turn: TurnId, wake: bool) -> Option<&MailboxEntry> {
        self.messages
            .values()
            .filter(|entry| {
                entry.message.receiver == receiver
                    && !matches!(entry.status, MessageStatus::Consumed(_))
                    && entry.status != MessageStatus::Claimed(turn)
                    && (!wake || entry.message.completion.is_none())
            })
            .min_by_key(|entry| entry.queued_at)
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

    pub fn wake_inputs(&self, receiver: AgentId) -> Result<Vec<AgentMessage>, ConversationFailure> {
        let mut pending: Vec<_> = self
            .messages
            .values()
            .filter(|entry| {
                entry.message.receiver == receiver
                    && !matches!(entry.status, MessageStatus::Consumed(_))
                    && entry.message.completion.is_none()
            })
            .collect();
        pending.sort_unstable_by_key(|entry| entry.queued_at);
        let Some(trigger) = pending.iter().position(|entry| {
            entry.status == MessageStatus::Pending
                && entry.message.mode == DeliveryMode::TriggerTurn
        }) else {
            return Ok(Vec::new());
        };
        pending[..=trigger]
            .iter()
            .map(|entry| {
                if entry.status != MessageStatus::Pending {
                    return Err(ConversationFailure::new(
                        "Agent 有尚未确认的领取记录，须先恢复",
                    ));
                }
                Ok(entry.message.clone())
            })
            .collect()
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
                    || message.text.chars().any(|character| {
                        character.is_control() && !matches!(character, '\t' | '\r' | '\n')
                    })
                {
                    return Err(ConversationFailure::new("消息缺少有效身份、来源或正文"));
                }
                if let Some(completion) = &message.completion {
                    let valid = message.mode == DeliveryMode::QueueOnly
                        && completion.agent_turn.0 > 0
                        && matches!(
                            completion.state,
                            AgentState::Completed | AgentState::Interrupted | AgentState::Failed(_)
                        )
                        && self.find(completion.task).is_some_and(|entry| {
                            entry.message.receiver == message.sender
                                && entry.message.sender == completion.assigned_by
                                && entry.message.mode == DeliveryMode::TriggerTurn
                                && entry.message.turn == message.turn
                                && entry.message.call_id == message.call_id
                                && entry.message.text == completion.assignment
                        })
                        && !self.messages.values().any(|entry| {
                            entry.message.sender == message.sender
                                && entry
                                    .message
                                    .completion
                                    .as_ref()
                                    .is_some_and(|prior| prior.agent_turn == completion.agent_turn)
                        });
                    if valid {
                        return Ok(None);
                    }
                    return Err(ConversationFailure::new(
                        "子 Agent 完成结果缺少唯一的任务来源",
                    ));
                }
                let pending = self
                    .messages
                    .values()
                    .filter(|entry| {
                        entry.message.receiver == message.receiver
                            && !matches!(entry.status, MessageStatus::Consumed(_))
                            && entry.message.completion.is_none()
                    })
                    .count();
                if pending >= MAX_PENDING_PER_AGENT {
                    return Err(ConversationFailure::new("接收方 Mailbox 已满"));
                }
            }
            MailboxEvent::Claimed { claim } => {
                if claim.turn.0 == 0
                    || !self
                        .next_input(claim.receiver, claim.turn, claim.wake)
                        .is_some_and(|entry| {
                            entry.message.id == claim.message
                                && entry.status == MessageStatus::Pending
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
