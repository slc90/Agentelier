use crate::{BackgroundBinding, SessionId, SessionView, ToolStatus, TurnStatus, TurnView};
use serde::{Deserialize, Serialize};

pub const BACKGROUND_RESULT_TOOL: &str = "record_background_result";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundStatus {
    Pending,
    Running,
    NeedsInput,
    Failed,
    Interrupted,
    Unconfirmed,
    Succeeded,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackgroundTask {
    pub session: SessionId,
    pub source: SessionId,
    pub title: String,
    pub status: BackgroundStatus,
    pub detail: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackgroundOutcome {
    pub completed: bool,
    pub summary: String,
}

impl BackgroundTask {
    pub fn from_view(view: &SessionView) -> Option<Self> {
        let binding = view.settings.background.as_ref()?;
        Some(Self::from_record(
            view.id,
            binding,
            view.turn.as_deref().or_else(|| view.previous_turns.last()),
            false,
            view.execution_blocked,
        ))
    }

    pub fn from_record(
        session: SessionId,
        binding: &BackgroundBinding,
        turn: Option<&TurnView>,
        restoring: bool,
        blocked: bool,
    ) -> Self {
        let (status, detail) = task_status(turn, restoring, blocked);
        Self {
            session,
            source: binding.source,
            title: binding.title.clone(),
            status,
            detail,
        }
    }
}

fn task_status(
    turn: Option<&TurnView>,
    restoring: bool,
    blocked: bool,
) -> (BackgroundStatus, Option<String>) {
    let Some(turn) = turn else {
        return (BackgroundStatus::Pending, None);
    };
    if blocked
        || turn.tools.iter().any(|tool| {
            matches!(
                tool.status,
                ToolStatus::Unconfirmed | ToolStatus::ExternalUnconfirmed
            )
        })
    {
        return (
            BackgroundStatus::Unconfirmed,
            Some("上次工具结果或执行收尾未确认".into()),
        );
    }
    let active_tool = turn.tools.iter().any(|tool| {
        matches!(
            tool.status,
            ToolStatus::Receiving | ToolStatus::Pending | ToolStatus::Running
        )
    });
    let outcome = turn.tools.iter().rev().find_map(|tool| {
        (tool.name == BACKGROUND_RESULT_TOOL && tool.status == ToolStatus::Succeeded)
            .then(|| serde_json::from_str::<BackgroundOutcome>(&tool.arguments).ok())
            .flatten()
    });
    match &turn.status {
        TurnStatus::Running if active_tool && restoring => (
            BackgroundStatus::Unconfirmed,
            Some("上次执行中断，工具结果未确认".into()),
        ),
        TurnStatus::Running if restoring => {
            (BackgroundStatus::Interrupted, Some("上次执行中断".into()))
        }
        TurnStatus::Running => (BackgroundStatus::Running, None),
        TurnStatus::Completed if active_tool && restoring => (
            BackgroundStatus::Unconfirmed,
            Some("应用退出前未保存后台工具结果".into()),
        ),
        TurnStatus::Completed if active_tool => (BackgroundStatus::Running, None),
        TurnStatus::Completed => match outcome {
            Some(outcome) if outcome.completed => {
                (BackgroundStatus::Succeeded, Some(outcome.summary))
            }
            Some(outcome) => (BackgroundStatus::NeedsInput, Some(outcome.summary)),
            None => (
                BackgroundStatus::NeedsInput,
                Some("本轮已结束，等待原聊天处理结果".into()),
            ),
        },
        TurnStatus::Failed(reason) => (BackgroundStatus::Failed, Some(reason.clone())),
        TurnStatus::Limited => (
            BackgroundStatus::Failed,
            Some("本轮达到请求或工具预算".into()),
        ),
        TurnStatus::Cancelled => (BackgroundStatus::Stopped, None),
    }
}
