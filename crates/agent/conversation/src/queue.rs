use crate::{Conversation, ConversationFailure, SessionId, SessionView, TurnId, TurnStatus};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct QueuedInput {
    pub operation_id: u64,
    pub input_id: TurnId,
    #[serde(flatten)]
    pub submission: crate::ComposerMessage,
}
pub enum SessionCommand {
    Enqueue {
        id: u64,
        session: SessionId,
        input: crate::ComposerSubmission,
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
    pub fn session(&self) -> SessionId {
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
                if matches!(self.decision, QueueDecision::Enqueued)
                    && self.state.queue.last().is_some_and(|input| {
                        input
                            .submission
                            .uses_audio_draft(session.view.audio_draft.as_ref())
                    })
                {
                    session.view.audio_draft = None;
                }
                session.view.queue = self.state.queue;
                session.view.queue_paused = self.state.queue_paused
                    || session.view.execution_blocked
                    || !session.view.task_policy.external_unconfirmed.is_empty()
                    || (session.view.queue_paused
                        && (!matches!(self.decision, QueueDecision::Resumed)
                            || self.pause_revision != pause_revision));
                if matches!(self.decision, QueueDecision::Resumed) && !session.view.queue_paused {
                    session.view.task_policy.background_paused = false;
                }
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
        let id = command.id();
        let mut next_turn = self.next_turn;
        let Some(mut view) = self.current() else {
            return Err(Box::new(QueueEvent {
                id,
                view: None,
                decision: QueueDecision::Rejected("排队会话已变化".into()),
            }));
        };
        let decision = Self::change_queue(&mut view, &mut next_turn, command)
            .unwrap_or_else(|failure| QueueDecision::Rejected(failure.to_string()));
        if matches!(
            decision,
            QueueDecision::Rejected(_) | QueueDecision::AlreadyStarted { .. }
        ) {
            return Err(Box::new(QueueEvent {
                id,
                view: Some(view),
                decision,
            }));
        }
        Ok(PreparedQueue {
            id,
            decision,
            state: crate::StoredState::from(&view),
            next_turn,
            pause_revision: self.queue_pause_revision,
        })
    }

    fn change_queue(
        view: &mut SessionView,
        next_turn: &mut u64,
        command: SessionCommand,
    ) -> Result<QueueDecision, ConversationFailure> {
        if view.id != command.session() {
            return Err(ConversationFailure::new("排队会话已变化"));
        }
        if view.execution_blocked {
            return Err(ConversationFailure::new(
                "会话执行记录不可用，请重置或新建会话",
            ));
        }
        match command {
            SessionCommand::Enqueue { id, input, .. } => {
                let submission = input.resolve(view)?;
                let input_id = next_turn
                    .checked_add(1)
                    .ok_or(ConversationFailure::new("输入标识已耗尽"))?;
                *next_turn = input_id;
                view.queue.push(QueuedInput {
                    operation_id: id,
                    input_id: TurnId(input_id),
                    submission,
                });
                Ok(QueueDecision::Enqueued)
            }
            SessionCommand::Remove { operation, .. } => {
                if let Some(index) = view
                    .queue
                    .iter()
                    .position(|input| input.operation_id == operation)
                {
                    view.queue.remove(index);
                    Ok(QueueDecision::Removed { operation })
                } else if view
                    .previous_turns
                    .iter()
                    .chain(view.turn.iter().map(Box::as_ref))
                    .any(|turn| turn.operation_id == operation)
                {
                    Ok(QueueDecision::AlreadyStarted { operation })
                } else {
                    Err(ConversationFailure::new("待办条目已不存在"))
                }
            }
            SessionCommand::Resume { .. } => {
                if !view.task_policy.external_unconfirmed.is_empty() {
                    return Err(ConversationFailure::new("须明确确认远端效果后才能继续队列"));
                }
                if view
                    .turn
                    .as_ref()
                    .is_some_and(|turn| matches!(turn.status, TurnStatus::Running))
                {
                    return Err(ConversationFailure::new("当前任务尚未回收，不能继续队列"));
                }
                view.queue_paused = false;
                Ok(QueueDecision::Resumed)
            }
        }
    }

    pub fn next_queued(&self) -> Option<&QueuedInput> {
        self.session
            .as_ref()
            .filter(|session| {
                !session.view.queue_paused
                    && !session.view.task_policy.background_paused
                    && session.view.task_policy.external_unconfirmed.is_empty()
                    && !session.view.execution_blocked
                    && session
                        .history
                        .as_ref()
                        .is_some_and(|history| !history.has_open_synchronous_calls())
                    && session
                        .view
                        .turn
                        .as_ref()
                        .is_none_or(|turn| !matches!(turn.status, TurnStatus::Running))
            })?
            .view
            .queue
            .first()
    }

    pub fn pause_invalid_queue(
        &mut self,
        reason: String,
    ) -> Result<SessionView, ConversationFailure> {
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| ConversationFailure::new("出队缺少会话"))?;
        session.view.queue_paused = true;
        session.view.notice = Some(format!("队列已暂停：{reason}"));
        Ok(session.view.clone())
    }

    pub fn start_queued(&mut self) -> Result<Option<(u64, SessionView)>, ConversationFailure> {
        let Some(input) = self.next_queued().cloned() else {
            return Ok(None);
        };
        let session_id = self
            .session_id()
            .ok_or_else(|| ConversationFailure::new("出队缺少会话"))?;
        self.begin_input(input.operation_id, session_id, Some(input.submission))?;
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure::new("出队缺少会话"))?;
        let queued = session.view.queue.remove(0);
        let admission = session
            .active
            .as_mut()
            .and_then(|active| active.admission.as_mut())
            .ok_or_else(|| ConversationFailure::new("出队缺少接纳状态"))?;
        admission.queued = Some(queued);
        session
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("出队缺少任务"))?
            .input_id = input.input_id;
        Ok(Some((input.operation_id, session.view.clone())))
    }
}
