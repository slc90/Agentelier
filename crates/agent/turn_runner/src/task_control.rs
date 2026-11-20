use crate::access::TurnAccess;
use agentelier_conversation::{ConversationFailure, TaskPage};
use agentelier_tools::{
    MAX_TOOL_BODY_BYTES, TaskControl, TaskControlRequest, TaskTargets, TaskWait, ToolResult,
    ToolStatus, WaitMode,
};
use serde::Serialize;
use std::time::Duration;
use tokio::time::Instant;

pub(super) enum TaskControlAction {
    Return(ToolResult),
    Wait(TaskWait),
    Cancel(TaskTargets),
}

pub(super) async fn control(
    active: &TurnAccess,
    dispatcher: &crate::ToolDispatcher,
    kind: TaskControl,
    arguments: &str,
) -> Result<TaskControlAction, ConversationFailure> {
    let request = match kind.parse(arguments) {
        Ok(request) => request,
        Err(error) => {
            return Ok(TaskControlAction::Return(ToolResult::error(
                ToolStatus::Failed,
                error.code,
                &error.to_string(),
            )));
        }
    };
    let processes = if matches!(request, TaskControlRequest::List(_)) {
        dispatcher.processes()
    } else {
        Vec::new()
    };
    active
        .call(move |turn| match request {
            TaskControlRequest::List(list) => {
                let explicit = list.task_ids.is_some();
                let mut page = match list.task_ids {
                    Some(ids) => {
                        let Some(tasks) = turn.task_reports(&ids) else {
                            return Ok(TaskControlAction::Return(unknown_tasks()));
                        };
                        TaskPage {
                            tasks,
                            next_history_offset: None,
                            processes: Vec::new(),
                            blocked_by_process: None,
                        }
                    }
                    None => turn.task_page(list.history_offset),
                };
                let blocker = processes.iter().find(|process| {
                    matches!(
                        process.phase,
                        agentelier_tools::ProcessPhase::Running
                            | agentelier_tools::ProcessPhase::CleanupUnconfirmed
                    )
                });
                page.blocked_by_process = blocker.map(|process| process.session_id.clone());
                for report in &mut page.tasks {
                    report.process = processes
                        .iter()
                        .find(|process| process.origin.task_id == Some(report.task_id.0))
                        .cloned();
                }
                if !explicit {
                    let mut resources: Vec<_> = processes
                        .into_iter()
                        .filter(|process| {
                            !page.tasks.iter().any(|report| {
                                report.process.as_ref().is_some_and(|included| {
                                    included.session_id == process.session_id
                                })
                            })
                        })
                        .collect();
                    resources.sort_by_key(|process| {
                        (
                            !(process.readable
                                || process.phase == agentelier_tools::ProcessPhase::Running),
                            std::cmp::Reverse(process.origin.turn),
                            std::cmp::Reverse(process.origin.request),
                        )
                    });
                    page.processes.extend(
                        resources
                            .into_iter()
                            .take(agentelier_tools::MAX_PROCESS_SESSIONS),
                    );
                }
                Ok(TaskControlAction::Return(success(page)?))
            }
            TaskControlRequest::Cancel(targets) => Ok(TaskControlAction::Cancel(targets)),
            TaskControlRequest::Wait(wait) => Ok(if turn.task_selection_exists(&wait.task_ids) {
                TaskControlAction::Wait(wait)
            } else {
                TaskControlAction::Return(unknown_tasks())
            }),
        })
        .await
}

pub(super) fn unknown_tasks() -> ToolResult {
    ToolResult::error(
        ToolStatus::Failed,
        "unknown_task",
        "存在未知任务 ID，或该任务在本次请求冻结时尚不可引用",
    )
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WaitInterruption {
    Steered,
    Stopped,
}

pub(super) struct PendingWait {
    pub(super) call_id: String,
    pub(super) deadline: Instant,
    request: TaskWait,
}

#[derive(Serialize)]
struct WaitReport {
    ready: bool,
    timed_out: bool,
    interrupted_by: Option<WaitInterruption>,
    tasks: Vec<agentelier_conversation::TaskReport>,
}

impl PendingWait {
    pub(super) fn new(call_id: String, request: TaskWait) -> Self {
        Self {
            call_id,
            deadline: Instant::now() + Duration::from_millis(request.timeout_ms),
            request,
        }
    }

    pub(super) async fn evaluate(
        &self,
        active: &TurnAccess,
        interrupted_by: Option<WaitInterruption>,
    ) -> Result<Option<ToolResult>, ConversationFailure> {
        let timed_out = Instant::now() >= self.deadline;
        let ids = self.request.task_ids.clone();
        let mode = self.request.mode;
        active
            .call(move |turn| {
                let tasks = turn
                    .task_reports(&ids)
                    .ok_or_else(|| ConversationFailure::new("等待期间已冻结的任务目标丢失"))?;
                let ready = match mode {
                    WaitMode::Any => tasks.iter().any(|task| task.result_saved()),
                    WaitMode::All => tasks.iter().all(|task| task.result_saved()),
                };
                if ready || timed_out || interrupted_by.is_some() {
                    let mut output = success(WaitReport {
                        ready,
                        timed_out: !ready && timed_out && interrupted_by.is_none(),
                        interrupted_by,
                        tasks,
                    })?;
                    if interrupted_by.is_some() {
                        output.status = ToolStatus::Cancelled;
                    }
                    Ok(Some(output))
                } else {
                    Ok(None)
                }
            })
            .await
    }
}

pub(super) fn success(value: impl Serialize) -> Result<ToolResult, ConversationFailure> {
    let body = serde_json::to_string(&value).map_err(|failure| {
        ConversationFailure::new(format!("任务控制结果序列化失败：{failure}"))
    })?;
    if body.len() > MAX_TOOL_BODY_BYTES {
        return Ok(ToolResult::error(
            ToolStatus::Failed,
            "result_too_large",
            "任务控制结果超过大小限制",
        ));
    }
    Ok(ToolResult {
        status: ToolStatus::Succeeded,
        output: body.into(),
        cleanup_failed: false,
    })
}
