use crate::{ToolDefinition, ToolFailure};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

pub const MAX_TASK_PAGE: usize = 64;
const MAX_WAIT_MS: u64 = 30_000;

#[derive(Clone, Copy)]
pub enum TaskControl {
    List,
    Wait,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskList {
    pub task_ids: Option<Vec<Uuid>>,
    pub history_offset: Option<usize>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitMode {
    Any,
    All,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWait {
    pub task_ids: Vec<Uuid>,
    pub mode: WaitMode,
    pub timeout_ms: u64,
}

pub enum TaskControlRequest {
    List(TaskList),
    Wait(TaskWait),
}

impl TaskControl {
    pub(crate) fn definition(self) -> ToolDefinition {
        let (name, description, parameters) = match self {
            Self::List => (
                "list_async_tasks",
                "List asynchronous tasks in this conversation, including jobs launched in the same response. Use null for history_offset to prioritize pending delivery. To page through delivered tasks use the returned next_history_offset. Or pass explicit task_ids with a null history_offset. Returns safe status and task IDs, never the original tool outputs. Use wait_for_tasks with returned IDs if you need their outputs.",
                json!({"type":"object","properties":{
                    "task_ids":{"type":["array","null"],"items":{"type":"string"},"minItems":1,"maxItems":MAX_TASK_PAGE},
                    "history_offset":{"type":["integer","null"],"minimum":0}
                },"required":["task_ids","history_offset"],"additionalProperties":false}),
            ),
            Self::Wait => (
                "wait_for_tasks",
                "Wait for saved terminal results of the given task IDs from an earlier list_async_tasks result. Use any or all, with a timeout of at most 30000 ms. A timeout ends only the wait. Original results are delivered separately; this tool returns statuses.",
                json!({"type":"object","properties":{
                    "task_ids":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":MAX_TASK_PAGE},
                    "mode":{"type":"string","enum":["any","all"]},
                    "timeout_ms":{"type":"integer","minimum":0,"maximum":MAX_WAIT_MS}
                },"required":["task_ids","mode","timeout_ms"],"additionalProperties":false}),
            ),
        };
        ToolDefinition {
            name: name.into(),
            description: description.into(),
            parameters,
            source: crate::ToolSource::Builtin,
            strict: true,
            mode: crate::ToolMode::Synchronous,
            display_source: None,
            output_schema: None,
        }
    }

    pub fn parse(self, text: &str) -> Result<TaskControlRequest, ToolFailure> {
        if text.len() > crate::MAX_TOOL_BODY_BYTES {
            return Err(invalid("任务控制参数超过大小限制"));
        }
        match self {
            Self::List => {
                let request: TaskList =
                    crate::process::arguments(text, &["task_ids", "history_offset"])?;
                if let Some(ids) = &request.task_ids {
                    validate_ids(ids)?;
                    if request.history_offset.is_some() {
                        return Err(invalid("点名查询不能同时指定历史页位置"));
                    }
                }
                Ok(TaskControlRequest::List(request))
            }
            Self::Wait => {
                let request: TaskWait =
                    crate::process::arguments(text, &["task_ids", "mode", "timeout_ms"])?;
                validate_ids(&request.task_ids)?;
                if request.timeout_ms > MAX_WAIT_MS {
                    return Err(invalid("等待期限不能超过 30000 毫秒"));
                }
                Ok(TaskControlRequest::Wait(request))
            }
        }
    }
}

fn invalid(message: &str) -> ToolFailure {
    ToolFailure::new("invalid_arguments", message)
}

fn validate_ids(ids: &[Uuid]) -> Result<(), ToolFailure> {
    if ids.is_empty() || ids.len() > MAX_TASK_PAGE {
        return Err(invalid("目标集合须包含 1 至 64 项任务 ID"));
    }
    let mut unique = std::collections::HashSet::with_capacity(ids.len());
    if !ids.iter().all(|id| unique.insert(id)) {
        return Err(invalid("目标集合包含重复任务 ID"));
    }
    Ok(())
}
