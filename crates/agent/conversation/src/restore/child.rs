use super::{ReplaySeed, RestoredSession, replay};
use crate::{
    AgentCreationRecord, AgentId, AgentIdentity, AgentState, ChildConfigSnapshot, Conversation,
    ConversationFailure, InitialSession, MailboxEvent, PreparedChild, RecordEvent, SessionId,
    SessionRecord, StoredImage,
};
use agentelier_tools::{ImagePayload, WorkingDirectory};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

pub struct RestoredChild {
    pub identity: AgentIdentity,
    pub settle_unknown: bool,
    config: ChildConfigSnapshot,
    thread: RestoredSession,
}

impl RestoredChild {
    pub fn recovery_event(&self) -> Option<RecordEvent> {
        self.thread.normalized.then(|| self.thread.recovery_event())
    }

    pub fn confirm_recovery(&mut self, sequence: u64) {
        self.thread.confirm_recovery(sequence);
    }
}

pub fn restore_child(
    session: SessionId,
    agent: AgentId,
    records: Vec<SessionRecord>,
    mut load: impl FnMut(&StoredImage) -> Result<Arc<ImagePayload>, ConversationFailure>,
) -> Result<RestoredChild, ConversationFailure> {
    let root = AgentId::for_root(session);
    let mut creations = HashMap::new();
    let mut history = Vec::new();
    let mut mail = HashMap::new();
    let mut state = AgentState::Starting;
    for record in records {
        match record.event {
            RecordEvent::AgentCreated { creation, .. } => {
                if creations.insert(creation.identity.id, creation).is_some() {
                    return Err(ConversationFailure::new(
                        "冷恢复的存档包含重复子 Agent 身份",
                    ));
                }
            }
            RecordEvent::AgentExecution {
                agent: id,
                state: current,
                delivery,
                ..
            } => {
                if id == agent {
                    state = current;
                }
                if let Some(message) = delivery
                    && message.receiver == agent
                {
                    mail.insert(message.id, *message);
                }
            }
            RecordEvent::AgentStopRequested { stop, .. } if stop.affected.contains(&agent) => {
                state = AgentState::Stopping;
            }
            RecordEvent::AgentTurn {
                agent: id, event, ..
            } if id == agent => {
                history.push(SessionRecord {
                    sequence: record.sequence,
                    session: event.session(),
                    at_ms: record.at_ms,
                    event: *event,
                });
            }
            RecordEvent::Mailbox {
                event: MailboxEvent::Queued { message },
                ..
            } if message.receiver == agent => {
                mail.insert(message.id, message);
            }
            _ => {}
        }
    }
    if matches!(state, AgentState::Stopping | AgentState::Closed) {
        return Err(ConversationFailure::new(
            "子 Agent 停止或关闭尚未允许重新执行",
        ));
    }
    let creation = creations
        .remove(&agent)
        .ok_or_else(|| ConversationFailure::new("子 Agent 冷恢复缺少持久身份"))?;
    let mut child = creation.as_ref();
    loop {
        child.validate(session)?;
        let parent_id = child
            .identity
            .parent
            .ok_or_else(|| ConversationFailure::new("冷恢复的子 Agent 缺少父级"))?;
        let fork = &child.config.fork;
        if parent_id == root {
            if fork.parent_thread != session || child.identity.depth != 1 {
                return Err(ConversationFailure::new(
                    "冷恢复的子树根路径或来源线程不一致",
                ));
            }
            break;
        }
        let parent = creations.get(&parent_id).filter(|parent| {
            parent.identity.depth.checked_add(1) == Some(child.identity.depth)
                && parent.identity.path.contains(&child.identity.path)
                && fork.parent_thread == parent.identity.thread.session_id()
        });
        child = parent
            .map(Box::as_ref)
            .ok_or_else(|| ConversationFailure::new("冷恢复的祖先身份、路径或来源线程不一致"))?;
    }
    let AgentCreationRecord {
        identity,
        config,
        history: initial_history,
        ..
    } = *creation;
    let initial = InitialSession {
        session: identity.thread.session_id(),
        title: identity.nickname.clone(),
        created_at_ms: 0,
        settings: config.settings.clone(),
    };
    let seed = ReplaySeed {
        history: initial_history,
        publications: config.projects.iter().cloned().map(Arc::new).collect(),
        child_mail: Some(mail),
    };
    let thread = replay(initial, history, seed, |reference| {
        load(&StoredImage {
            session,
            ..reference.clone()
        })
    })?;
    Ok(RestoredChild {
        identity,
        settle_unknown: matches!(state, AgentState::Starting | AgentState::Running),
        config,
        thread,
    })
}

impl Conversation {
    pub fn install_child(&self, restored: RestoredChild) -> Result<Self, ConversationFailure> {
        let RestoredChild {
            identity,
            mut config,
            thread,
            ..
        } = restored;
        let session = self
            .session
            .as_ref()
            .filter(|session| session.view.id == identity.session)
            .ok_or_else(|| ConversationFailure::new("子 Agent 所属会话在恢复期间已变化"))?;
        let saved = &config.settings;
        if saved.codex != session.view.settings.codex {
            return Err(ConversationFailure::new(
                "子 Agent 冻结的执行身份与当前会话不一致",
            ));
        }
        WorkingDirectory::validate(&saved.working_directory).map_err(failure)?;
        let allowed: BTreeSet<_> = saved.tools.iter().map(|tool| tool.name.clone()).collect();
        let tools = session
            .tools
            .inherited(&allowed, saved.tool_contract, saved.image_input)
            .rebind(&saved.tools)
            .map_err(failure)?;
        config.projects = thread
            .publications
            .iter()
            .map(|project| project.as_ref().clone())
            .collect();
        let mut child = Self::from_child(
            identity.thread.session_id(),
            PreparedChild {
                config,
                history: thread.history,
                tools: Arc::new(tools),
            },
        )?;
        let install = child
            .session
            .as_mut()
            .ok_or_else(|| ConversationFailure::new("冷恢复缺少子会话状态"))?;
        let mut state = thread.state;
        for turn in state
            .previous_turns
            .iter_mut()
            .chain(state.turn.iter_mut().map(Box::as_mut))
        {
            turn.operation_id = 0;
        }
        if state.turn.is_some() || !state.previous_turns.is_empty() {
            install.view.context = state.context;
        }
        install.view.turn = state.turn;
        install.view.previous_turns = state.previous_turns;
        install.view.execution_blocked = state.execution_blocked;
        install.view.notice = thread.notice;
        *install.view.task_policy = state.task_policy;
        install.processes = thread.processes;
        install.tasks = thread.tasks;
        child.next_turn = thread.next_turn;
        Ok(child)
    }
}

fn failure(error: impl std::fmt::Display) -> ConversationFailure {
    ConversationFailure::new(error.to_string())
}
