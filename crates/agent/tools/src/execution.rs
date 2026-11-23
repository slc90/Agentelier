use crate::ToolCatalog;
use serde_json::json;
use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub const MAX_TOOL_BODY_BYTES: usize = 64 * 1024;
const ERROR_MESSAGE_BYTES: usize = 8192;

#[derive(Clone)]
pub struct ToolUserTurn {
    pub memory_epoch: Option<i64>,
    pub memory_management: bool,
    pub session: uuid::Uuid,
    pub turn: u64,
    pub input: u64,
    pub statements: Vec<String>,
}

pub struct ExecutionContext {
    pub(crate) directory: PathBuf,
    image_input: Option<bool>,
    user_turn: Arc<ToolUserTurn>,
    request: u32,
}

impl ExecutionContext {
    pub fn new(
        directory: PathBuf,
        image_input: Option<bool>,
        user_turn: Arc<ToolUserTurn>,
        request: u32,
    ) -> Self {
        Self {
            directory,
            image_input,
            user_turn,
            request,
        }
    }

    pub fn user_turn(&self) -> &ToolUserTurn {
        &self.user_turn
    }

    pub fn image_input(&self) -> Option<bool> {
        self.image_input
    }

    pub fn request(&self) -> u32 {
        self.request
    }

    pub fn working_directory(&self) -> &Path {
        &self.directory
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolStatus {
    Unconfirmed,
    ExternalUnconfirmed,
    Receiving,
    Pending,
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    NotExecuted,
}

#[derive(Clone, Debug)]
pub struct ToolFailure {
    pub code: &'static str,
    message: String,
}
impl ToolFailure {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for ToolFailure {}

#[derive(Clone)]
pub struct ToolResult {
    pub status: ToolStatus,
    pub output: crate::ToolPayload,
    pub cleanup_failed: bool,
}
impl ToolResult {
    pub fn reject_image_budget(self) -> Self {
        let message = format!(
            "图片已读取，但加入会话将超过 16 MiB 图片预算，未保留图片载荷。读取元数据：{}",
            self.output.summary()
        );
        Self::error(ToolStatus::Failed, "session_image_limit", &message)
    }
    pub fn not_executed(code: &str, message: &str) -> Self {
        Self::error(ToolStatus::NotExecuted, code, message)
    }
    pub fn error(status: ToolStatus, code: &str, message: &str) -> Self {
        let mut end = message.len().min(ERROR_MESSAGE_BYTES);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let message = &message[..end];
        Self {
            status,
            cleanup_failed: false,
            output: json!({"ok":false,"error":{"code":code,"message":message}})
                .to_string()
                .into(),
        }
    }
}

pub async fn execute(
    catalog: &ToolCatalog,
    context: &ExecutionContext,
    processes: &crate::ProcessSessions,
    scripts: &crate::ScriptSession,
    name: &str,
    arguments: &str,
    cancel: &CancellationToken,
    permit: crate::ToolPermit,
    origin: crate::ProcessOrigin,
) -> ToolResult {
    if cancel.is_cancelled() {
        return ToolResult::not_executed("stopped", "任务已停止，工具未执行");
    }
    if arguments.len() > MAX_TOOL_BODY_BYTES {
        return ToolResult::error(
            ToolStatus::Failed,
            "arguments_too_large",
            "工具参数超过大小限制",
        );
    }
    let Some(tool) = catalog.find(name) else {
        return ToolResult::error(ToolStatus::Failed, "unknown_tool", "未登记的工具名称");
    };
    tool.execute(
        context, processes, scripts, arguments, cancel, permit, origin,
    )
    .await
}

pub(crate) async fn image(
    context: &ExecutionContext,
    arguments: &str,
    cancel: &CancellationToken,
    supported: Option<bool>,
) -> ToolResult {
    if supported == Some(false) {
        return ToolResult::error(
            ToolStatus::Failed,
            "image_capability",
            "模型目录明确不支持图片输入，未读取图片",
        );
    }
    crate::image::run(arguments, context.directory.clone(), cancel).await
}
