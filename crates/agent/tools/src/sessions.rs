use crate::ToolAdmission;
use crate::execution::{ToolFailure, ToolResult, ToolStatus};
use crate::process::{PROCESS_POLL, Process, arguments};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, OwnedRwLockWriteGuard};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const MAX_PROCESS_SESSIONS: usize = 8;
const DEFAULT_YIELD_MS: u64 = 10_000;
const MAX_YIELD_MS: u64 = 30_000;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ProcessCleanup {
    pub session_id: String,
    pub cleanup_confirmed: bool,
    pub output: String,
}
pub struct CleanupReport {
    pub processes: Vec<ProcessCleanup>,
    pub failed: bool,
}
pub struct ProcessSessions {
    registry: Mutex<ProcessRegistry>,
    admission: ToolAdmission,
}

#[derive(Default)]
struct ProcessRegistry {
    processes: BTreeMap<String, Arc<ProcessEntry>>,
    watchers: Vec<JoinHandle<()>>,
}

struct ProcessEntry {
    state: Mutex<ProcessState>,
    terminate: CancellationToken,
}

enum ProcessState {
    Running {
        process: Box<Process>,
        _lease: OwnedRwLockWriteGuard<()>,
    },
    Exited(ToolResult),
    Consumed,
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

impl ProcessSessions {
    pub fn new(admission: ToolAdmission) -> Self {
        Self {
            registry: Mutex::default(),
            admission,
        }
    }

    pub(crate) async fn exec(
        &self,
        directory: &Path,
        text: &str,
        cancel: &CancellationToken,
        lease: OwnedRwLockWriteGuard<()>,
    ) -> ToolResult {
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
        if self.registry.lock().await.processes.len() >= MAX_PROCESS_SESSIONS {
            return ToolResult::error(
                ToolStatus::Failed,
                "session_limit",
                "任务最多持有 8 个未回收进程会话，请先结束已有会话",
            );
        }
        let mut process = match Process::start(&input.cmd, input.workdir, directory, cancel).await {
            Ok(process) => process,
            Err(error) => return failure(error),
        };
        let id = uuid::Uuid::new_v4().to_string();
        tracing::debug!(session_id = id, "本地进程会话已启动");
        let (result, completed) = poll(
            &mut process,
            &id,
            PollInput {
                wait,
                terminate: false,
                error: None,
                evidence: json!({"written_bytes":0,"write_complete":true,"close_stdin":false,"terminate":false}),
            },
            cancel,
            &CancellationToken::new(),
        )
        .await;
        if result.cleanup_failed {
            self.admission.block();
        }
        if !completed {
            let entry = Arc::new(ProcessEntry {
                state: Mutex::new(ProcessState::Running {
                    process: Box::new(process),
                    _lease: lease,
                }),
                terminate: CancellationToken::new(),
            });
            let mut registry = self.registry.lock().await;
            registry.processes.insert(id.clone(), entry.clone());
            registry.watchers.push(tokio::spawn(watch_exit(
                Arc::downgrade(&entry),
                id,
                self.admission.clone(),
            )));
        }
        result
    }

    pub(crate) async fn stdin(&self, text: &str, cancel: &CancellationToken) -> ToolResult {
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
        let entry = self
            .registry
            .lock()
            .await
            .processes
            .get(&input.session_id)
            .cloned();
        let Some(entry) = entry else {
            return unknown();
        };
        if input.terminate {
            entry.terminate.cancel();
        }
        let mut locked = entry.state.lock().await;
        let process = match &mut *locked {
            ProcessState::Running { process, .. } => process,
            ProcessState::Exited(result) => {
                if !input.chars.is_empty() {
                    return ToolResult::error(
                        ToolStatus::Failed,
                        "process_exited",
                        "进程已退出，未写入标准输入；空轮询可查看退出结果",
                    );
                }
                let result = result.clone();
                *locked = ProcessState::Consumed;
                self.registry
                    .lock()
                    .await
                    .processes
                    .remove(&input.session_id);
                return result;
            }
            ProcessState::Consumed => return unknown(),
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
                    _ = entry.terminate.cancelled() => {
                        write_error = Some("进程终止已请求；仅保留已确认写入的字节".into());
                        break;
                    },
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
        let (result, completed) = if write_cancelled {
            (
                complete(
                    process,
                    &input.session_id,
                    true,
                    ToolStatus::Cancelled,
                    write_error,
                    state,
                )
                .await,
                true,
            )
        } else {
            poll(
                process,
                &input.session_id,
                PollInput {
                    wait,
                    terminate: input.terminate,
                    error: write_error,
                    evidence: state,
                },
                cancel,
                &entry.terminate,
            )
            .await
        };
        if result.cleanup_failed {
            self.admission.block();
        }
        if completed {
            *locked = ProcessState::Consumed;
            self.registry
                .lock()
                .await
                .processes
                .remove(&input.session_id);
        }
        result
    }

    pub async fn cleanup(&self, reason: &str) -> CleanupReport {
        let mut report = CleanupReport {
            processes: Vec::new(),
            failed: false,
        };
        let ProcessRegistry {
            processes,
            watchers,
        } = std::mem::take(&mut *self.registry.lock().await);
        for (id, entry) in processes {
            entry.terminate.cancel();
            let mut locked = entry.state.lock().await;
            let result = match &mut *locked {
                ProcessState::Running { process, .. } => {
                    complete(
                        process,
                        &id,
                        true,
                        ToolStatus::Succeeded,
                        None,
                        json!({"automatic_cleanup":true,"reason":reason}),
                    )
                    .await
                }
                ProcessState::Exited(result) => result.clone(),
                ProcessState::Consumed => continue,
            };
            report.failed |= result.cleanup_failed;
            report.processes.push(ProcessCleanup {
                session_id: id,
                cleanup_confirmed: !result.cleanup_failed,
                output: result.output.summary().to_owned(),
            });
            *locked = ProcessState::Consumed;
        }
        for watcher in watchers {
            if let Err(failure) = watcher.await {
                report.failed = true;
                tracing::error!(%failure, "进程回收观察者异常结束");
            }
        }
        report
    }
}

fn unknown() -> ToolResult {
    ToolResult::error(
        ToolStatus::Failed,
        "unknown_session",
        "进程会话不属于当前任务、已回收或不存在，命令未重放",
    )
}

async fn watch_exit(entry: Weak<ProcessEntry>, id: String, admission: ToolAdmission) {
    loop {
        tokio::time::sleep(PROCESS_POLL).await;
        let Some(entry) = entry.upgrade() else {
            return;
        };
        if entry.terminate.is_cancelled() {
            return;
        }
        let mut locked = entry.state.lock().await;
        let ProcessState::Running { process, .. } = &mut *locked else {
            return;
        };
        let (status, error) = match process.empty() {
            Ok(false) => continue,
            Ok(true) => (ToolStatus::Succeeded, None),
            Err(failure) => (ToolStatus::Failed, Some(failure.to_string())),
        };
        let result = complete(
            process,
            &id,
            false,
            status,
            error,
            json!({"process_exit_observed":true}),
        )
        .await;
        if result.cleanup_failed {
            admission.block();
        }
        *locked = ProcessState::Exited(result);
        return;
    }
}

struct PollInput {
    wait: Duration,
    terminate: bool,
    error: Option<String>,
    evidence: Value,
}

async fn poll(
    process: &mut Process,
    id: &str,
    input: PollInput,
    cancel: &CancellationToken,
    terminate_requested: &CancellationToken,
) -> (ToolResult, bool) {
    let PollInput {
        wait,
        terminate,
        mut error,
        evidence,
    } = input;
    let mut status = if error.is_some() {
        ToolStatus::Failed
    } else {
        ToolStatus::Succeeded
    };
    let mut completed = terminate;
    if !terminate {
        let outcome = tokio::select! { biased;
            _ = terminate_requested.cancelled() => {
                return (process.result(ToolStatus::Cancelled, None,
                    error.or_else(|| Some("轮询被该进程的终止请求打断".into())),
                    false, id, evidence, false), false);
            },
            result = tokio::time::timeout(wait, process.wait_empty()) => result,
            _ = cancel.cancelled() => {
                status = ToolStatus::Cancelled;
                completed = true;
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
        complete(process, id, terminate, status, error, evidence).await
    } else {
        process.result(status, None, error, false, id, evidence, false)
    };
    (result, completed)
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
