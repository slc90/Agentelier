use crate::Transport;
use crate::connections::{Failure, Update, serve};
use process_wrap::tokio::{ChildWrapper, CommandWrap, CreationFlags, JobObject, KillOnDrop};
use rmcp::transport::async_rw::AsyncRwTransport;
use std::{
    env, io,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, ReadBuf};
use tokio_util::sync::CancellationToken;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

const PROCESS_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

pub(crate) async fn run(
    transport: Transport,
    stop: CancellationToken,
    publish: impl Fn(Update),
) -> Result<(), Failure> {
    let Transport::Stdio {
        command,
        args,
        working_directory,
        environment,
    } = transport
    else {
        return Err(Failure {
            stage: "启动",
            reason: "暂未支持该传输类型",
        });
    };
    let mut child = spawn(&command, &args, &working_directory, &environment)?;
    let protocol = match (child.stdout().take(), child.stdin().take()) {
        (Some(stdout), Some(stdin)) => {
            serve(
                AsyncRwTransport::new_client(BoundedRead::new(stdout), stdin),
                &stop,
                publish,
            )
            .await
        }
        _ => Err(Failure {
            stage: "启动",
            reason: "未取得 MCP 协议管道",
        }),
    };
    reap(child).await?;
    protocol
}

fn spawn(
    command: &Path,
    args: &[String],
    directory: &Path,
    environment: &[String],
) -> Result<Box<dyn ChildWrapper>, Failure> {
    let binary = resolve_program(command).ok_or(Failure {
        stage: "启动",
        reason: "找不到配置的 MCP 可执行文件",
    })?;
    if !directory.is_dir() {
        return Err(Failure {
            stage: "启动",
            reason: "配置的启动目录不存在或不可访问",
        });
    }
    let mut command = CommandWrap::with_new(binary, |cmd| {
        cmd.args(args)
            .current_dir(directory)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for name in environment.iter().map(String::as_str).chain(["SystemRoot"]) {
            if let Some(value) = env::var_os(name) {
                cmd.env(name, value);
            }
        }
    });
    command
        .wrap(KillOnDrop)
        .wrap(CreationFlags(CREATE_NO_WINDOW))
        .wrap(JobObject);
    command.spawn().map_err(|_| Failure {
        stage: "启动",
        reason: "Windows 拒绝启动 MCP 子进程或建立进程组",
    })
}

fn resolve_program(command: &Path) -> Option<PathBuf> {
    if command.is_absolute() {
        return command.is_file().then(|| command.to_path_buf());
    }
    let command = command.with_extension("exe");
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(&command))
        .find(|file| file.is_file())
}

async fn reap(mut child: Box<dyn ChildWrapper>) -> Result<(), Failure> {
    let failure = Failure {
        stage: "回收",
        reason: "无法确认 MCP 子进程退出",
    };
    let root_exited = matches!(
        tokio::time::timeout(PROCESS_CLOSE_TIMEOUT, child.inner_mut().wait()).await,
        Ok(Ok(_))
    );
    if let Err(error) = child.start_kill()
        && (!root_exited || error.kind() != io::ErrorKind::PermissionDenied)
    {
        return Err(failure);
    }
    tokio::time::timeout(PROCESS_CLOSE_TIMEOUT, child.wait())
        .await
        .ok()
        .and_then(Result::ok)
        .map(|_| ())
        .ok_or(failure)
}

struct BoundedRead<R> {
    inner: R,
    line_bytes: usize,
}

impl<R> BoundedRead<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            line_bytes: 0,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for BoundedRead<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let previous = buffer.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buffer) {
            Poll::Ready(Ok(())) => {
                for byte in &buffer.filled()[previous..] {
                    if *byte == b'\n' {
                        self.line_bytes = 0;
                    } else {
                        self.line_bytes += 1;
                        if self.line_bytes > MAX_FRAME_BYTES {
                            return Poll::Ready(Err(io::Error::other("MCP 消息帧超过 2 MiB")));
                        }
                    }
                }
                Poll::Ready(Ok(()))
            }
            result => result,
        }
    }
}
