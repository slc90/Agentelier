use crate::execution::{ToolFailure, ToolResult, ToolStatus};
use crate::process::{Process, arguments};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

const MAX_PROCESS_SESSIONS: usize = 8;
const DEFAULT_YIELD_MS: u64 = 10_000;
const MAX_YIELD_MS: u64 = 30_000;

#[derive(Clone)]
pub struct ProcessCleanup {
    pub session_id: String,
    pub cleanup_confirmed: bool,
    pub output: String,
}
pub struct CleanupReport {
    pub processes: Vec<ProcessCleanup>,
    pub failed: bool,
}
pub struct ExecutionContext {
    pub(crate) directory: PathBuf,
    processes: BTreeMap<String, Process>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecArguments {
    cmd: String,
    workdir: Option<PathBuf>,
    yield_time_ms: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StdinArguments {
    session_id: String,
    chars: String,
    yield_time_ms: Option<u64>,
    close_stdin: bool,
    terminate: bool,
}

fn yield_duration(value: Option<u64>) -> Result<Duration, ToolFailure> {
    let value = value.unwrap_or(DEFAULT_YIELD_MS);
    if value > MAX_YIELD_MS {
        return Err(ToolFailure::new(
            "invalid_arguments",
            "yield_time_ms 必须为 0 至 30000 毫秒",
        ));
    }
    Ok(Duration::from_millis(value))
}
fn failure(error: ToolFailure) -> ToolResult {
    ToolResult::error(
        if error.code == "cancelled" {
            ToolStatus::Cancelled
        } else {
            ToolStatus::Failed
        },
        error.code,
        &error.to_string(),
    )
}

impl ExecutionContext {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            processes: BTreeMap::new(),
        }
    }

    pub(crate) async fn exec(&mut self, text: &str, cancel: &CancellationToken) -> ToolResult {
        let input: ExecArguments = match arguments(text, &["cmd", "workdir", "yield_time_ms"]) {
            Ok(value) => value,
            Err(error) => return failure(error),
        };
        let wait = match yield_duration(input.yield_time_ms) {
            Ok(value) => value,
            Err(error) => return failure(error),
        };
        if input.cmd.trim().is_empty() {
            return ToolResult::error(ToolStatus::Failed, "invalid_arguments", "命令不能为空");
        }
        if self.processes.len() >= MAX_PROCESS_SESSIONS {
            return ToolResult::error(
                ToolStatus::Failed,
                "session_limit",
                "任务最多持有 8 个未回收进程会话，请先结束已有会话",
            );
        }
        let process = match Process::start(&input.cmd, input.workdir, &self.directory, cancel).await
        {
            Ok(process) => process,
            Err(error) => return failure(error),
        };
        let id = uuid::Uuid::new_v4().to_string();
        tracing::debug!(session_id = id, "本地进程会话已启动");
        self.processes.insert(id.clone(), process);
        self.poll(
            &id,
            wait,
            false,
            None,
            json!({"written_bytes":0,"write_complete":true,"close_stdin":false,"terminate":false}),
            cancel,
        )
        .await
    }

    pub(crate) async fn stdin(&mut self, text: &str, cancel: &CancellationToken) -> ToolResult {
        let input: StdinArguments = match arguments(
            text,
            &[
                "session_id",
                "chars",
                "yield_time_ms",
                "close_stdin",
                "terminate",
            ],
        ) {
            Ok(value) => value,
            Err(error) => return failure(error),
        };
        let wait = match yield_duration(input.yield_time_ms) {
            Ok(value) => value,
            Err(error) => return failure(error),
        };
        if input.terminate && (!input.chars.is_empty() || input.close_stdin) {
            return ToolResult::error(
                ToolStatus::Failed,
                "invalid_arguments",
                "terminate 不能与非空 chars 或 close_stdin 同时使用",
            );
        }
        let Some(process) = self.processes.get_mut(&input.session_id) else {
            return ToolResult::error(
                ToolStatus::Failed,
                "unknown_session",
                "进程会话不属于当前任务、已回收或不存在，命令未重放",
            );
        };
        let mut written = 0;
        let mut write_error = None;
        let mut write_cancelled = false;
        if !input.chars.is_empty() {
            let Some(stdin) = process.stdin.as_mut() else {
                return ToolResult::error(
                    ToolStatus::Failed,
                    "stdin_closed",
                    "该进程会话的标准输入已关闭，输入未写入",
                );
            };
            let bytes = input.chars.as_bytes();
            while written < bytes.len() {
                let next = tokio::select! {biased;
                    result = stdin.write(&bytes[written..]) => result,
                    _ = cancel.cancelled() => { write_cancelled = true; write_error = Some("输入已取消；只报告已确认的写入字节数，不自动重放输入，不能确认对端处理进度".into()); break; },
                };
                match next {
                    Ok(0) => {
                        write_error = Some("标准输入零字节写入，未完成剩余输入".into());
                        break;
                    }
                    Ok(length) => written += length,
                    Err(error) => {
                        write_error = Some(format!("标准输入写入失败：{error}；不自动重放输入"));
                        break;
                    }
                }
            }
        }
        if input.close_stdin && write_error.is_none() {
            process.stdin.take();
        }
        let state = json!({"written_bytes":written,"write_complete":written == input.chars.len(),"close_stdin":input.close_stdin && write_error.is_none(),"terminate":input.terminate});
        if write_cancelled {
            let result = complete(
                process,
                &input.session_id,
                true,
                ToolStatus::Cancelled,
                write_error,
                state,
            )
            .await;
            self.processes.remove(&input.session_id);
            return result;
        }
        self.poll(
            &input.session_id,
            wait,
            input.terminate,
            write_error,
            state,
            cancel,
        )
        .await
    }

    async fn poll(
        &mut self,
        id: &str,
        wait: Duration,
        terminate: bool,
        mut error: Option<String>,
        input: Value,
        cancel: &CancellationToken,
    ) -> ToolResult {
        let Some(process) = self.processes.get_mut(id) else {
            return ToolResult::error(ToolStatus::Failed, "unknown_session", "进程会话已回收");
        };
        let mut status = if error.is_some() {
            ToolStatus::Failed
        } else {
            ToolStatus::Succeeded
        };
        let mut completed = terminate;
        if !terminate {
            let outcome = tokio::select! {biased;
                result = tokio::time::timeout(wait, process.wait_empty()) => result,
                _ = cancel.cancelled() => {
                    status = ToolStatus::Cancelled; completed = true;
                    error = Some(format!("{}；任务停止，关联进程组已请求终止",error.unwrap_or_default()));
                    Ok(Ok(()))
                },
            };
            match outcome {
                Ok(Ok(())) => completed = true,
                Ok(Err(failure)) => {
                    status = ToolStatus::Failed;
                    error = Some(failure.to_string());
                    completed = true;
                }
                Err(_) => {}
            }
        }
        if !completed {
            match process.empty() {
                Ok(empty) => completed = empty,
                Err(failure) => {
                    status = ToolStatus::Failed;
                    error = Some(failure.to_string());
                    completed = true;
                }
            }
        }
        let result = if completed {
            complete(process, id, terminate, status, error, input).await
        } else {
            process.result(status, None, error, false, id, input, false)
        };
        if completed {
            self.processes.remove(id);
        }
        result
    }

    pub async fn cleanup(&mut self, reason: &str) -> CleanupReport {
        let mut report = CleanupReport {
            processes: Vec::new(),
            failed: false,
        };
        for (id, mut process) in std::mem::take(&mut self.processes) {
            let result = complete(
                &mut process,
                &id,
                true,
                ToolStatus::Succeeded,
                None,
                json!({"automatic_cleanup":true,"reason":reason}),
            )
            .await;
            report.failed |= result.cleanup_failed || result.status == ToolStatus::Failed;
            report.processes.push(ProcessCleanup {
                session_id: id,
                cleanup_confirmed: !result.cleanup_failed,
                output: result.output.summary().to_owned(),
            });
        }
        report
    }
}

async fn complete(
    process: &mut Process,
    id: &str,
    terminate: bool,
    mut status: ToolStatus,
    mut error: Option<String>,
    input: Value,
) -> ToolResult {
    process.stdin.take();
    let reaped = process
        .reap(terminate || status != ToolStatus::Succeeded)
        .await;
    let readers = process.readers().await;
    let cleanup_failed = reaped.is_err();
    let exit = match reaped {
        Ok(exit) => {
            if status == ToolStatus::Succeeded && !terminate && !exit.success() {
                status = ToolStatus::Failed;
                error = Some("命令以非零退出码结束".into());
            }
            Some(exit)
        }
        Err(failure) => {
            status = ToolStatus::Failed;
            error = Some(format!("进程回收失败：{failure}"));
            None
        }
    };
    if let Err(failure) = readers {
        status = ToolStatus::Failed;
        error = Some(format!(
            "{}；输出收尾失败：{failure}",
            error.unwrap_or_default()
        ));
    }
    if !cleanup_failed && let Some(message) = error.as_mut() {
        *message = message.replace("关联进程组已请求终止", "关联进程组已终止并回收");
    }
    tracing::debug!(
        session_id = id,
        cleanup_confirmed = !cleanup_failed,
        error = error.as_deref(),
        "本地进程会话收尾已返回"
    );
    process.result(status, exit, error, cleanup_failed, id, input, true)
}
