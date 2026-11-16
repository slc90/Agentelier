use crate::{ToolCatalog, diagnostic};
use serde_json::json;
use std::{error::Error, fmt, time::Duration};
use tokio_util::sync::CancellationToken;

pub const MAX_TOOL_BODY_BYTES: usize = 64 * 1024;
const TOOL_TIMEOUT: Duration = Duration::from_secs(15);
const ERROR_MESSAGE_BYTES: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Receiving,
    Pending,
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
    NotExecuted,
}

#[derive(Debug)]
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
    pub(crate) fn error(status: ToolStatus, code: &str, message: &str) -> Self {
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
    context: &mut crate::ExecutionContext,
    name: &str,
    arguments: &str,
    cancel: &CancellationToken,
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
    let Some(implementation) = catalog.find(name) else {
        return ToolResult::error(ToolStatus::Failed, "unknown_tool", "未登记的工具名称");
    };
    let implementation = match implementation {
        crate::catalog::Implementation::Exec => return context.exec(arguments, cancel).await,
        crate::catalog::Implementation::Stdin => return context.stdin(arguments, cancel).await,
        crate::catalog::Implementation::Patch => {
            return crate::patch::run(arguments, context.directory.clone(), cancel).await;
        }
        crate::catalog::Implementation::Image { supported } => {
            if supported == Some(false) {
                return ToolResult::error(
                    ToolStatus::Failed,
                    "image_capability",
                    "模型目录明确不支持图片输入，未读取图片",
                );
            }
            return crate::image::run(arguments, context.directory.clone(), cancel).await;
        }
        crate::catalog::Implementation::Diagnostic(implementation) => implementation,
    };
    let result = tokio::select! { biased;
        result = tokio::time::timeout(TOOL_TIMEOUT, diagnostic::run(implementation, arguments)) => result,
        _ = cancel.cancelled() => return ToolResult::error(ToolStatus::Cancelled, "cancelled", "工具执行已取消"),
    };
    match result {
        Ok(Ok(value)) => {
            let output = json!({"ok":true, "data":value}).to_string();
            if output.len() > MAX_TOOL_BODY_BYTES {
                ToolResult::error(
                    ToolStatus::Failed,
                    "result_too_large",
                    "工具结果超过大小限制",
                )
            } else {
                ToolResult {
                    status: ToolStatus::Succeeded,
                    cleanup_failed: false,
                    output: output.into(),
                }
            }
        }
        Ok(Err(failure)) => ToolResult::error(ToolStatus::Failed, failure.code, &failure.message),
        Err(_) => ToolResult::error(
            ToolStatus::TimedOut,
            "timeout",
            "工具执行超过 15 秒期限，执行已结束",
        ),
    }
}
