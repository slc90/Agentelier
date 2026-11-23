use crate::{SessionId, TaskDelivery, TaskId};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GlobalTaskStatus {
    Waiting,
    Running,
    WaitingForResources,
    Cancelling,
    Failed,
    Cancelled,
    ResultReady,
    NeedsConfirmation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GlobalTaskProcess {
    None,
    Running,
    Exited,
    Unconfirmed,
}

#[derive(Clone, PartialEq, Eq)]
pub struct GlobalTask {
    pub id: TaskId,
    pub name: String,
    pub status: GlobalTaskStatus,
    pub delivery: TaskDelivery,
    pub process: GlobalTaskProcess,
}

#[derive(Clone, PartialEq, Eq)]
pub struct GlobalTaskGroup {
    pub session: SessionId,
    pub title: String,
    pub tasks: Vec<GlobalTask>,
}

pub struct GlobalTasks {
    pub revision: u64,
    pub groups: Vec<GlobalTaskGroup>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TaskNotificationKind {
    Succeeded,
    Failed,
    NeedsConfirmation,
}

pub struct TaskNotification {
    pub task: TaskId,
    pub session: SessionId,
    pub session_title: String,
    pub name: String,
    pub kind: TaskNotificationKind,
}
