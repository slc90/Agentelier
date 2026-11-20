use crate::execution::{ToolFailure, ToolResult, ToolStatus};
use crate::process::{PROCESS_POLL, Process, arguments};
use crate::{ProcessChange, ProcessFact, ProcessOrigin, SaveProcessFact, ToolAdmission};
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

pub const MAX_PROCESS_SESSIONS: usize = 8;
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
    save: SaveProcessFact,
}

#[derive(Clone, Copy)]
pub enum ProcessScope {
    Turn(u64),
    All,
}

#[derive(Default)]
struct ProcessRegistry {
    processes: BTreeMap<String, Arc<ProcessEntry>>,
    watchers: BTreeMap<String, JoinHandle<()>>,
}

struct ProcessEntry {
    state: Mutex<ProcessState>,
    terminate: CancellationToken,
    origin: ProcessOrigin,
}

enum ProcessState {
    Running {
        process: Box<Process>,
        _lease: OwnedRwLockWriteGuard<()>,
    },
    Exited(ToolResult),
    Unconfirmed {
        _result: ToolResult,
        _process: Option<Box<Process>>,
        _lease: OwnedRwLockWriteGuard<()>,
    },
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
    pub fn new(admission: ToolAdmission, save: SaveProcessFact) -> Self {
        Self {
            registry: Mutex::default(),
            admission,
            save,
        }
    }

    pub(crate) async fn exec(
        &self,
        directory: &Path,
        text: &str,
        cancel: &CancellationToken,
        lease: OwnedRwLockWriteGuard<()>,
        origin: ProcessOrigin,
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
                "会话最多持有 8 个存活或未读取退出结果的进程，请先读取或收起退出结果",
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
                kill_on_cancel: true,
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
                origin,
            });
            if let Err(error) = save(&self.save, &entry, &id, ProcessChange::Started).await {
                let mut locked = entry.state.lock().await;
                let _cleanup = reap_entry(
                    &entry,
                    &id,
                    &mut locked,
                    &self.save,
                    &self.admission,
                    "registration_failed",
                )
                .await;
                return storage_failure(error);
            }
            let mut registry = self.registry.lock().await;
            registry.processes.insert(id.clone(), entry.clone());
            if cancel.is_cancelled() {
                drop(registry);
                let mut locked = entry.state.lock().await;
                let ProcessState::Running { process, .. } = &mut *locked else {
                    self.admission.block();
                    return ToolResult::error(
                        ToolStatus::Unconfirmed,
                        "process_identity",
                        "取消时创建进程的状态已变化",
                    );
                };
                let cancelled = complete(
                    process,
                    &id,
                    true,
                    ToolStatus::Cancelled,
                    Some("创建任务已取消，关联进程组已请求终止".into()),
                    json!({"terminate":true,"reason":"launch_cancelled"}),
                )
                .await;
                if let Err(error) = finish(
                    &entry,
                    &id,
                    &mut locked,
                    &self.save,
                    &self.admission,
                    cancelled.clone(),
                )
                .await
                {
                    return storage_failure(error);
                }
                if !cancelled.cleanup_failed
                    && let Err(error) = self.consume(&entry, &id, &mut locked).await
                {
                    return storage_failure(error);
                }
                return cancelled;
            }
            registry.watchers.retain(|_, handle| !handle.is_finished());
            registry.watchers.insert(
                id.clone(),
                tokio::spawn(watch_exit(
                    Arc::downgrade(&entry),
                    id,
                    self.admission.clone(),
                    self.save.clone(),
                )),
            );
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
        let mut locked = if input.terminate {
            entry.state.lock().await
        } else {
            tokio::select! { biased;
                _ = cancel.cancelled() => return ToolResult::not_executed("stopped", "控制已取消，未读取或写入进程"),
                locked = entry.state.lock() => locked,
            }
        };
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
                return match self.consume(&entry, &input.session_id, &mut locked).await {
                    Ok(()) => result,
                    Err(error) => storage_failure(error),
                };
            }
            ProcessState::Unconfirmed { .. } => {
                return ToolResult::error(
                    ToolStatus::Failed,
                    "process_unconfirmed",
                    "退出或保存未确认，进程结果暂不可读取",
                );
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
                process.result(
                    ToolStatus::Cancelled,
                    None,
                    write_error,
                    false,
                    &input.session_id,
                    state,
                    false,
                ),
                false,
            )
        } else {
            poll(
                process,
                &input.session_id,
                PollInput {
                    wait,
                    terminate: input.terminate,
                    kill_on_cancel: false,
                    error: write_error,
                    evidence: state,
                },
                cancel,
                &entry.terminate,
            )
            .await
        };
        if completed {
            if let Err(error) = finish(
                &entry,
                &input.session_id,
                &mut locked,
                &self.save,
                &self.admission,
                result.clone(),
            )
            .await
            {
                return storage_failure(error);
            }
            if !result.cleanup_failed
                && let Err(error) = self.consume(&entry, &input.session_id, &mut locked).await
            {
                return storage_failure(error);
            }
        }
        result
    }

    async fn consume(
        &self,
        entry: &ProcessEntry,
        id: &str,
        state: &mut ProcessState,
    ) -> std::io::Result<()> {
        if let Err(error) = save(&self.save, entry, id, ProcessChange::Consumed).await {
            self.admission.block();
            return Err(error);
        }
        *state = ProcessState::Consumed;
        self.registry.lock().await.processes.remove(id);
        Ok(())
    }

    pub async fn terminate(&self, id: &str) -> Result<(), String> {
        let entry = self
            .registry
            .lock()
            .await
            .processes
            .get(id)
            .cloned()
            .ok_or_else(|| "进程不属于当前会话或已经移除".to_owned())?;
        entry.terminate.cancel();
        let mut state = entry.state.lock().await;
        match reap_entry(
            &entry,
            id,
            &mut state,
            &self.save,
            &self.admission,
            "user_terminate",
        )
        .await
        {
            Some(report) if report.cleanup_confirmed => Ok(()),
            None if matches!(*state, ProcessState::Exited(_)) => Ok(()),
            _ => Err("进程终止回收或保存未确认".into()),
        }
    }

    pub async fn discard(&self, id: &str) -> Result<(), String> {
        let entry = self
            .registry
            .lock()
            .await
            .processes
            .get(id)
            .cloned()
            .ok_or_else(|| "进程不属于当前会话或已经移除".to_owned())?;
        let mut state = entry.state.lock().await;
        if !matches!(*state, ProcessState::Exited(_)) {
            return Err("仅可收起已确认退出的进程结果".into());
        }
        self.consume(&entry, id, &mut state)
            .await
            .map_err(|error| error.to_string())
    }

    pub async fn cleanup(&self, reason: &str, scope: ProcessScope) -> CleanupReport {
        let mut report = CleanupReport {
            processes: Vec::new(),
            failed: false,
        };
        let processes: Vec<_> = self
            .registry
            .lock()
            .await
            .processes
            .iter()
            .filter(|(_, entry)| match scope {
                ProcessScope::All => true,
                ProcessScope::Turn(turn) => entry.origin.turn == turn,
            })
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect();
        for (_, entry) in &processes {
            entry.terminate.cancel();
        }
        for (id, entry) in processes {
            let mut locked = entry.state.lock().await;
            if let Some(cleanup) = reap_entry(
                &entry,
                &id,
                &mut locked,
                &self.save,
                &self.admission,
                reason,
            )
            .await
            {
                report.failed |= !cleanup.cleanup_confirmed;
                report.processes.push(cleanup);
            }
            report.failed |= matches!(*locked, ProcessState::Unconfirmed { .. });
            drop(locked);
            let watcher = self.registry.lock().await.watchers.remove(&id);
            if let Some(watcher) = watcher
                && let Err(failure) = watcher.await
            {
                report.failed = true;
                tracing::error!(%failure, "进程回收观察者异常结束");
            }
        }
        report
    }
}

async fn save(
    save: &SaveProcessFact,
    entry: &ProcessEntry,
    id: &str,
    change: ProcessChange,
) -> std::io::Result<()> {
    save(ProcessFact {
        session_id: id.to_owned(),
        origin: entry.origin.clone(),
        change,
    })
    .await
}

fn storage_failure(error: std::io::Error) -> ToolResult {
    let mut result = ToolResult::error(
        ToolStatus::Unconfirmed,
        "process_storage",
        &format!("进程事实保存未确认：{error}"),
    );
    result.cleanup_failed = true;
    result
}

async fn finish(
    entry: &ProcessEntry,
    id: &str,
    state: &mut ProcessState,
    journal: &SaveProcessFact,
    admission: &ToolAdmission,
    result: ToolResult,
) -> std::io::Result<()> {
    let saved = save(journal, entry, id, ProcessChange::exited(&result)).await;
    if saved.is_ok() && !result.cleanup_failed {
        *state = ProcessState::Exited(result);
    } else {
        admission.block();
        if let ProcessState::Running { process, _lease } =
            std::mem::replace(state, ProcessState::Consumed)
        {
            *state = ProcessState::Unconfirmed {
                _process: result.cleanup_failed.then_some(process),
                _lease,
                _result: result,
            };
        }
    }
    saved
}

async fn reap_entry(
    entry: &ProcessEntry,
    id: &str,
    state: &mut ProcessState,
    journal: &SaveProcessFact,
    admission: &ToolAdmission,
    reason: &str,
) -> Option<ProcessCleanup> {
    let ProcessState::Running { process, .. } = state else {
        return None;
    };
    let result = complete(
        process,
        id,
        true,
        ToolStatus::Succeeded,
        None,
        json!({"automatic_cleanup":true,"reason":reason}),
    )
    .await;
    let saved = finish(entry, id, state, journal, admission, result.clone()).await;
    if let Err(error) = &saved {
        tracing::error!(session_id = id, %error, "进程退出事实保存失败");
    }
    Some(ProcessCleanup {
        session_id: id.to_owned(),
        cleanup_confirmed: !result.cleanup_failed && saved.is_ok(),
        output: result.output.summary().to_owned(),
    })
}

fn unknown() -> ToolResult {
    ToolResult::error(
        ToolStatus::Failed,
        "unknown_session",
        "进程会话不属于当前会话、已消费或不存在，命令未重放",
    )
}

async fn watch_exit(
    entry: Weak<ProcessEntry>,
    id: String,
    admission: ToolAdmission,
    journal: SaveProcessFact,
) {
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
        if let Err(error) = finish(&entry, &id, &mut locked, &journal, &admission, result).await {
            tracing::error!(session_id = id, %error, "进程退出事实保存失败");
        }
        return;
    }
}

struct PollInput {
    wait: Duration,
    terminate: bool,
    kill_on_cancel: bool,
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
        kill_on_cancel,
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
                if !kill_on_cancel {
                    return (process.result(status, None, error.or_else(|| Some("本次轮询已取消，进程继续运行".into())),
                        false, id, evidence, false), false);
                }
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
