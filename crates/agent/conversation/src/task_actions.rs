use crate::{SessionId, TaskId};
use agentelier_tools::ProcessView;
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq)]
pub enum TaskAction {
    Cancel { task: TaskId, instance: Uuid },
    Terminate { process: String, instance: Uuid },
    ReadExit { process: String, instance: Uuid },
    DiscardExit { process: String, instance: Uuid },
    StopAll,
    Resume,
    ConfirmExternal { call_ids: Vec<String> },
}

pub struct TaskActionReceipt {
    pub session: SessionId,
    pub result: Result<TaskActionOutcome, String>,
}

pub enum TaskActionOutcome {
    Status(String),
    ProcessOutput(String),
}

pub struct SessionProcesses {
    pub session: SessionId,
    pub revision: u64,
    pub processes: Vec<ProcessView>,
}
