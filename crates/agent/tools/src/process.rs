use crate::WorkingDirectory;
use crate::execution::{MAX_TOOL_BODY_BYTES, ToolFailure, ToolResult, ToolStatus};
use crate::output::Reader;
use process_wrap::tokio::{
    ChildWrapper, CommandWrap, CommandWrapper, CreationFlags, JobObject, KillOnDrop,
};
use serde::Deserialize;
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;
use win32job::{ExtendedLimitInfo, Job};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

const COMMAND_LIFETIME: Duration = Duration::from_secs(15);
const REAP_DEADLINE: Duration = Duration::from_secs(10);
const PROCESS_POLL: Duration = Duration::from_millis(40);
const RESULT_STREAM_BYTES: usize = 16 * 1024;

#[derive(Debug)]
struct JobMonitor(Arc<Job>);
impl CommandWrapper for JobMonitor {
    fn post_spawn(
        &mut self,
        _: &mut Command,
        child: &mut Child,
        _: &CommandWrap,
    ) -> io::Result<()> {
        // JobObject's pre_spawn suspends the root before any child can escape this monitor.
        let handle = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("命令缺少进程句柄"))?;
        self.0
            .assign_process(handle as isize)
            .map_err(io::Error::other)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecArguments {
    cmd: String,
    workdir: Option<PathBuf>,
}

pub(crate) fn arguments<T: serde::de::DeserializeOwned>(
    text: &str,
    required: &[&str],
) -> Result<T, ToolFailure> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|error| {
        ToolFailure::new(
            "invalid_arguments",
            format!("工具参数不是有效 JSON：{error}"),
        )
    })?;
    if !value
        .as_object()
        .is_some_and(|fields| required.iter().all(|key| fields.contains_key(*key)))
    {
        return Err(ToolFailure::new(
            "invalid_arguments",
            "缺少工具必填字段，默认值也必须显式传 null",
        ));
    }
    serde_json::from_value(value).map_err(|error| {
        ToolFailure::new(
            "invalid_arguments",
            format!("工具参数类型或字段无效：{error}"),
        )
    })
}

pub(crate) struct Process {
    pub(crate) child: Box<dyn ChildWrapper>,
    monitor: Arc<Job>,
    pub(crate) stdout: Reader,
    pub(crate) stderr: Reader,
    pub(crate) directory: PathBuf,
}

impl Process {
    async fn start(
        input: &ExecArguments,
        base: &Path,
        cancel: &CancellationToken,
    ) -> Result<Self, ToolFailure> {
        let base = base.to_path_buf();
        let candidate = input.workdir.clone();
        let setup = tokio::task::spawn_blocking(move || -> io::Result<(PathBuf, PathBuf)> {
            let directory = WorkingDirectory::validate(&base)?
                .directory(candidate.as_deref())?
                .path()
                .to_path_buf();
            let shell = std::env::var_os("PATH")
                .and_then(|value| {
                    std::env::split_paths(&value)
                        .map(|entry| entry.join("pwsh.exe"))
                        .find(|path| path.is_file())
                })
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "PATH 中找不到 PowerShell 7 pwsh.exe",
                    )
                })?;
            Ok((directory, shell))
        })
        .await
        .map_err(|error| ToolFailure::new("environment_failed", error.to_string()))?
        .map_err(|error| {
            ToolFailure::new("environment_failed", format!("命令工作环境无效：{error}"))
        })?;
        let (directory, shell) = setup;
        if cancel.is_cancelled() {
            return Err(ToolFailure::new(
                "cancelled",
                "目录检查期间已停止，命令未启动",
            ));
        }
        let mut limits = ExtendedLimitInfo::default();
        limits.limit_kill_on_job_close();
        let monitor = Arc::new(
            Job::create_with_limit_info(&limits)
                .map_err(|error| ToolFailure::new("spawn_failed", error.to_string()))?,
        );
        let mut command = CommandWrap::with_new(shell, |command| {
            command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
                .arg(format!("[Console]::InputEncoding=[Text.UTF8Encoding]::new($false); [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $OutputEncoding=[Console]::OutputEncoding; {}", input.cmd))
                .current_dir(&directory).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        });
        command
            .wrap(KillOnDrop)
            .wrap(CreationFlags(CREATE_NO_WINDOW))
            .wrap(JobMonitor(monitor.clone()))
            .wrap(JobObject);
        let mut child = command.spawn().map_err(|error| {
            ToolFailure::new("spawn_failed", format!("无法启动 PowerShell 7：{error}"))
        })?;
        drop(command);
        let stdout = child
            .stdout()
            .take()
            .ok_or_else(|| ToolFailure::new("spawn_failed", "命令缺少 stdout 管道"))?;
        let stderr = child
            .stderr()
            .take()
            .ok_or_else(|| ToolFailure::new("spawn_failed", "命令缺少 stderr 管道"))?;
        Ok(Self {
            child,
            monitor,
            stdout: Reader::start(stdout),
            stderr: Reader::start(stderr),
            directory,
        })
    }

    fn empty(&self) -> io::Result<bool> {
        self.monitor
            .query_basic_and_io_accounting_info()
            .map(|info| info.active_processes == 0)
            .map_err(io::Error::other)
    }

    async fn wait_empty(&self) -> io::Result<()> {
        loop {
            if self.empty()? {
                return Ok(());
            }
            tokio::time::sleep(PROCESS_POLL).await;
        }
    }

    async fn reap(&mut self, terminate: bool) -> io::Result<ExitStatus> {
        if terminate && !self.empty()? {
            self.child.start_kill()?;
        }
        tokio::time::timeout(REAP_DEADLINE, self.wait_empty())
            .await
            .map_err(|_| io::Error::other("进程组尚未回收，无法确认执行环境已清理"))??;
        // Calling process-wrap wait only once, after the whole Job is empty, avoids its cached root-exit path.
        self.child.wait().await
    }

    async fn readers(&mut self) -> io::Result<()> {
        let result = tokio::time::timeout(REAP_DEADLINE, async {
            let (stdout, stderr) = tokio::join!(&mut self.stdout.task, &mut self.stderr.task);
            let stdout = stdout.map_err(io::Error::other).and_then(|result| result);
            let stderr = stderr.map_err(io::Error::other).and_then(|result| result);
            stdout.and(stderr)
        })
        .await;
        match result {
            Ok(result) => result,
            Err(_) => {
                self.stdout.task.abort();
                self.stderr.task.abort();
                let (stdout, stderr) = tokio::join!(&mut self.stdout.task, &mut self.stderr.task);
                Err(io::Error::other(format!(
                    "进程输出读取未能在回收期限内结束；stdout={stdout:?}；stderr={stderr:?}"
                )))
            }
        }
    }

    fn result(
        &self,
        status: ToolStatus,
        exit: Option<ExitStatus>,
        failure: Option<String>,
        cleanup_failed: bool,
    ) -> ToolResult {
        let captures = self
            .stdout
            .output
            .lock()
            .and_then(|stdout| self.stderr.output.lock().map(|stderr| (stdout, stderr)));
        let Ok((stdout, stderr)) = captures else {
            let mut result =
                ToolResult::error(ToolStatus::Failed, "output_failed", "命令输出缓冲状态损坏");
            result.cleanup_failed = cleanup_failed;
            return result;
        };
        let mut limit = RESULT_STREAM_BYTES;
        loop {
            let (out, out_discarded, out_replaced) = stdout.text(limit);
            let (err, err_discarded, err_replaced) = stderr.text(limit);
            let output = json!({"ok":status == ToolStatus::Succeeded,"data":{
                "state":if cleanup_failed {"cleanup_unconfirmed"} else {"exited"},
                "workdir":self.directory,"exit_code":exit.and_then(|code| code.code()), "cleanup_confirmed":!cleanup_failed,
                "stdout":out,"stderr":err,"truncated":out_discarded + err_discarded > 0,
                "discarded_bytes":out_discarded + err_discarded,"decoding_replaced":out_replaced || err_replaced,
                "error":failure
            }}).to_string();
            if output.len() <= MAX_TOOL_BODY_BYTES {
                return ToolResult {
                    status,
                    output,
                    cleanup_failed,
                };
            }
            if limit == 0 {
                let mut result = ToolResult::error(
                    ToolStatus::Failed,
                    "result_too_large",
                    "命令元数据超过结果大小限制",
                );
                result.cleanup_failed = cleanup_failed;
                return result;
            }
            limit /= 2;
        }
    }
}

pub(crate) async fn execute(text: &str, base: &Path, cancel: &CancellationToken) -> ToolResult {
    let input: ExecArguments = match arguments(text, &["cmd", "workdir"]) {
        Ok(input) => input,
        Err(error) => {
            return ToolResult::error(
                if error.code == "cancelled" {
                    ToolStatus::Cancelled
                } else {
                    ToolStatus::Failed
                },
                error.code,
                &error.to_string(),
            );
        }
    };
    if input.cmd.trim().is_empty() {
        return ToolResult::error(ToolStatus::Failed, "invalid_arguments", "命令不能为空");
    }
    let mut process = match Process::start(&input, base, cancel).await {
        Ok(process) => process,
        Err(error) => {
            return ToolResult::error(
                if error.code == "cancelled" {
                    ToolStatus::Cancelled
                } else {
                    ToolStatus::Failed
                },
                error.code,
                &error.to_string(),
            );
        }
    };
    let outcome = tokio::select! { biased;
        result = tokio::time::timeout(COMMAND_LIFETIME, process.wait_empty()) => match result {
            Ok(Ok(())) => (ToolStatus::Succeeded, None),
            Ok(Err(error)) => (ToolStatus::Failed, Some(error.to_string())),
            Err(_) => (ToolStatus::TimedOut, Some("命令执行超过 15 秒，进程组已请求终止".into())),
        },
        _ = cancel.cancelled() => (ToolStatus::Cancelled, Some("命令已取消，进程组已请求终止".into())),
    };
    let (mut status, mut failure) = outcome;
    if matches!(status, ToolStatus::Cancelled | ToolStatus::TimedOut)
        && process.empty().is_ok_and(|empty| empty)
    {
        status = ToolStatus::Succeeded;
        failure = None;
    }
    let reaped = process.reap(status != ToolStatus::Succeeded).await;
    let readers = process.readers().await;
    let cleanup_failed = reaped.is_err();
    let exit = match reaped {
        Ok(exit) => {
            if status == ToolStatus::Succeeded && !exit.success() {
                status = ToolStatus::Failed;
                failure = Some("命令以非零退出码结束".into());
            }
            Some(exit)
        }
        Err(error) => {
            status = ToolStatus::Failed;
            failure = Some(format!("进程回收失败：{error}"));
            None
        }
    };
    if let Err(error) = readers {
        status = ToolStatus::Failed;
        failure = Some(format!(
            "{}；输出收尾失败：{error}",
            failure.unwrap_or_default()
        ));
    }
    if !cleanup_failed && let Some(message) = failure.as_mut() {
        *message = message.replace("进程组已请求终止", "关联进程组已终止并回收");
    }
    process.result(status, exit, failure, cleanup_failed)
}
