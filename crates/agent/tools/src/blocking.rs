use crate::{ToolFailure, ToolResult, ToolStatus};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const FILE_WORK_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct Control {
    cancel: CancellationToken,
    deadline: Instant,
}
impl Control {
    pub(crate) fn new(cancel: CancellationToken) -> Self {
        Self {
            cancel,
            deadline: Instant::now() + FILE_WORK_TIMEOUT,
        }
    }

    pub(crate) fn check(&self) -> Result<(), ToolFailure> {
        if self.cancel.is_cancelled() {
            Err(ToolFailure::new(
                "cancelled",
                "已停止后续文件操作，已完成变化保留",
            ))
        } else if Instant::now() >= self.deadline {
            Err(ToolFailure::new(
                "timeout",
                "文件操作达到 15 秒协作式期限，已完成变化保留",
            ))
        } else {
            Ok(())
        }
    }
}

pub(crate) fn failure(error: ToolFailure) -> ToolResult {
    ToolResult::error(status(error.code), error.code, &error.to_string())
}

pub(crate) fn status(code: &str) -> ToolStatus {
    match code {
        "cancelled" => ToolStatus::Cancelled,
        "timeout" => ToolStatus::TimedOut,
        _ => ToolStatus::Failed,
    }
}

pub(crate) async fn run(
    cancel: &CancellationToken,
    work: impl FnOnce(Control) -> ToolResult + Send + 'static,
) -> ToolResult {
    let control = Control::new(cancel.clone());
    // Always join: dropping a blocking handle would leave unowned file work after task completion.
    match tokio::task::spawn_blocking(move || work(control)).await {
        Ok(result) => result,
        Err(error) => {
            ToolResult::error(ToolStatus::Failed, "file_worker_failed", &error.to_string())
        }
    }
}
