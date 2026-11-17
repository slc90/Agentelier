use crate::{
    ActiveTurn, Conversation, ConversationFailure, SessionId, SessionView, TurnId, TurnStatus,
};

pub const MAX_QUEUED_INPUTS: usize = 8;
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct QueuedInput {
    pub operation_id: u64,
    pub input_id: TurnId,
    pub text: String,
}
pub enum SessionCommand {
    Enqueue {
        id: u64,
        session: SessionId,
        text: String,
    },
    Remove {
        id: u64,
        session: SessionId,
        operation: u64,
    },
    Resume {
        id: u64,
        session: SessionId,
    },
}
impl SessionCommand {
    pub fn id(&self) -> u64 {
        match self {
            Self::Enqueue { id, .. } | Self::Remove { id, .. } | Self::Resume { id, .. } => *id,
        }
    }
    fn session(&self) -> SessionId {
        match self {
            Self::Enqueue { session, .. }
            | Self::Remove { session, .. }
            | Self::Resume { session, .. } => *session,
        }
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub enum QueueDecision {
    Enqueued,
    Removed { operation: u64 },
    AlreadyStarted { operation: u64 },
    Resumed,
    Rejected(String),
}
pub struct QueueEvent {
    pub id: u64,
    pub view: Option<SessionView>,
    pub decision: QueueDecision,
}

pub struct PreparedQueue {
    pub id: u64,
    pub decision: QueueDecision,
    state: crate::StoredState,
    next_turn: u64,
    pause_revision: u64,
}

impl PreparedQueue {
    pub fn session(&self) -> SessionId {
        self.state.session
    }
    pub fn record(&self) -> crate::RecordEvent {
        crate::RecordEvent::QueueChanged {
            state: self.state.clone(),
            decision: self.decision.clone(),
        }
    }

    pub fn commit(self, conversation: &mut Conversation) -> QueueEvent {
        let pause_revision = conversation.queue_pause_revision;
        match conversation
            .session
            .as_mut()
            .filter(|session| session.view.id == self.state.session)
        {
            Some(session) => {
                session.view.queue = self.state.queue;
                session.view.queue_paused = self.state.queue_paused
                    || session.view.execution_blocked
                    || (session.view.queue_paused
                        && (!matches!(self.decision, QueueDecision::Resumed)
                            || self.pause_revision != pause_revision));
                conversation.next_turn = conversation.next_turn.max(self.next_turn);
                QueueEvent {
                    id: self.id,
                    view: Some(session.view.clone()),
                    decision: self.decision,
                }
            }
            None => QueueEvent {
                id: self.id,
                view: None,
                decision: QueueDecision::Rejected("保存期间会话已变化，未应用待办操作".into()),
            },
        }
    }
}

impl Conversation {
    pub fn prepare_queue(&self, command: SessionCommand) -> Result<PreparedQueue, Box<QueueEvent>> {
        let mut candidate = self.clone();
        let event = candidate.queue_command(command);
        if matches!(
            event.decision,
            QueueDecision::Rejected(_) | QueueDecision::AlreadyStarted { .. }
        ) {
            return Err(Box::new(event));
        }
        let Some(view) = event.view else {
            return Err(Box::new(QueueEvent {
                id: event.id,
                view: None,
                decision: QueueDecision::Rejected("待办准备缺少会话".into()),
            }));
        };
        Ok(PreparedQueue {
            id: event.id,
            decision: event.decision,
            state: crate::StoredState::from(&view),
            next_turn: candidate.next_turn,
            pause_revision: self.queue_pause_revision,
        })
    }

    pub fn queue_command(&mut self, command: SessionCommand) -> QueueEvent {
        let id = command.id();
        let outcome = self.change_queue(command);
        QueueEvent {
            id,
            view: self.session.as_ref().map(|session| session.view.clone()),
            decision: outcome
                .unwrap_or_else(|failure| QueueDecision::Rejected(failure.to_string())),
        }
    }

    fn change_queue(
        &mut self,
        command: SessionCommand,
    ) -> Result<QueueDecision, ConversationFailure> {
        let session = self
            .session
            .as_mut()
            .filter(|session| session.view.id == command.session())
            .ok_or(ConversationFailure::new("排队会话已变化"))?;
        if session.view.execution_blocked {
            return Err(ConversationFailure::new(
                "会话执行记录不可用，请重置或新建会话",
            ));
        }
        match command {
            SessionCommand::Enqueue { id, text, .. } => {
                if text.trim().is_empty() {
                    return Err(ConversationFailure::new("不能排队空白消息"));
                }
                if session.view.queue.len() >= MAX_QUEUED_INPUTS {
                    return Err(ConversationFailure::new("待办消息已满，最多 8 条"));
                }
                let input_id = self
                    .next_turn
                    .checked_add(1)
                    .ok_or(ConversationFailure::new("输入标识已耗尽"))?;
                self.next_turn = input_id;
                session.view.queue.push(QueuedInput {
                    operation_id: id,
                    input_id: TurnId(input_id),
                    text,
                });
                Ok(QueueDecision::Enqueued)
            }
            SessionCommand::Remove { operation, .. } => {
                if let Some(index) = session
                    .view
                    .queue
                    .iter()
                    .position(|input| input.operation_id == operation)
                {
                    session.view.queue.remove(index);
                    Ok(QueueDecision::Removed { operation })
                } else if session
                    .view
                    .previous_turns
                    .iter()
                    .chain(session.view.turn.iter().map(Box::as_ref))
                    .any(|turn| turn.operation_id == operation)
                {
                    Ok(QueueDecision::AlreadyStarted { operation })
                } else {
                    Err(ConversationFailure::new("待办条目已不存在"))
                }
            }
            SessionCommand::Resume { .. } => {
                if session
                    .view
                    .turn
                    .as_ref()
                    .is_some_and(|turn| matches!(turn.status, TurnStatus::Running))
                {
                    return Err(ConversationFailure::new("当前任务尚未回收，不能继续队列"));
                }
                session.view.queue_paused = false;
                Ok(QueueDecision::Resumed)
            }
        }
    }

    pub fn start_queued(
        &mut self,
    ) -> Result<Option<(u64, SessionView, ActiveTurn)>, ConversationFailure> {
        let Some(session) = self.session.as_ref().filter(|session| {
            !session.view.queue_paused
                && !session.view.execution_blocked
                && session.history.is_some()
                && session
                    .view
                    .turn
                    .as_ref()
                    .is_none_or(|turn| !matches!(turn.status, TurnStatus::Running))
        }) else {
            return Ok(None);
        };
        let Some(input) = session.view.queue.first().cloned() else {
            return Ok(None);
        };
        let session_id = session.view.id;
        let (_, mut active) = self.begin(input.operation_id, session_id, input.text)?;
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("出队缺少会话"))?;
        session.view.queue.remove(0);
        session
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("出队缺少任务"))?
            .input_id = input.input_id;
        active.view = session.view.clone();
        Ok(Some((input.operation_id, session.view.clone(), active)))
    }
}
