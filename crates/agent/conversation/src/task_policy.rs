use crate::{Conversation, ConversationFailure, RecordEvent, SessionId, StoredState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct TaskPolicy {
    pub background_paused: bool,
    pub external_unconfirmed: BTreeSet<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskPolicyChange {
    Pause,
    Resume,
    ConfirmExternal { call_ids: Vec<String> },
}

impl TaskPolicy {
    pub(crate) fn apply(&mut self, change: &TaskPolicyChange) -> Result<(), ConversationFailure> {
        match change {
            TaskPolicyChange::Pause => self.background_paused = true,
            TaskPolicyChange::Resume => self.background_paused = false,
            TaskPolicyChange::ConfirmExternal { call_ids } => {
                let ids: BTreeSet<_> = call_ids.iter().collect();
                if ids.is_empty()
                    || ids.len() != call_ids.len()
                    || !ids.iter().all(|id| self.external_unconfirmed.contains(*id))
                {
                    return Err(ConversationFailure::new(
                        "远端效果确认范围为空、重复或已经变化",
                    ));
                }
                for id in ids {
                    self.external_unconfirmed.remove(id);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn record(
        &self,
        state: StoredState,
        change: TaskPolicyChange,
    ) -> Result<RecordEvent, ConversationFailure> {
        if state.execution_blocked && !matches!(change, TaskPolicyChange::Pause) {
            return Err(ConversationFailure::new("会话执行记录或资源回收未确认"));
        }
        self.clone().apply(&change)?;
        Ok(RecordEvent::TaskPolicyChanged { state, change })
    }
}

impl Conversation {
    pub fn idle_cancellation(
        &self,
        session: SessionId,
        targets: &agentelier_tools::TaskTargets,
    ) -> Option<Vec<crate::TaskCancelReceipt>> {
        let current = self
            .session
            .as_ref()
            .filter(|current| current.view.id == session && current.active.is_none())?;
        current
            .tasks
            .cancellation(targets, current.tasks.revision())
            .map(|plan| plan.receipts)
    }

    pub fn task_policy_record(
        &self,
        session: SessionId,
        change: TaskPolicyChange,
    ) -> Result<RecordEvent, ConversationFailure> {
        let current = self
            .session
            .as_ref()
            .filter(|current| current.view.id == session)
            .ok_or_else(|| ConversationFailure::new("任务控制会话已变化"))?;
        current
            .view
            .task_policy
            .record(StoredState::from(&current.view), change)
    }

    pub fn apply_task_policy(
        &mut self,
        session: SessionId,
        change: &TaskPolicyChange,
    ) -> Result<TaskPolicy, ConversationFailure> {
        let current = self
            .session
            .as_mut()
            .filter(|current| current.view.id == session)
            .ok_or_else(|| ConversationFailure::new("任务控制会话已变化"))?;
        current.view.task_policy.apply(change)?;
        if matches!(change, TaskPolicyChange::Pause) {
            current.view.queue_paused = true;
        }
        Ok(current.view.task_policy.as_ref().clone())
    }
}

impl crate::ActiveTurn<'_> {
    pub fn task_policy_record(
        &self,
        change: TaskPolicyChange,
    ) -> Result<RecordEvent, ConversationFailure> {
        self.view.task_policy.record(self.stored_state(), change)
    }

    pub fn apply_task_policy(
        &mut self,
        change: &TaskPolicyChange,
    ) -> Result<TaskPolicy, ConversationFailure> {
        self.view.task_policy.apply(change)?;
        if matches!(change, TaskPolicyChange::Pause) {
            self.view.queue_paused = true;
        }
        Ok(self.view.task_policy.as_ref().clone())
    }

    pub fn background_paused(&self) -> bool {
        self.view.task_policy.background_paused
    }
}
