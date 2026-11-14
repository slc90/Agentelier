use agentelier_login::{AuthFile, RestoreFailure, RestoredAccount};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::{error::Error, fmt, io};
use tokio::sync::mpsc;
use tracing::{debug, error};

const COMMAND_CAPACITY: usize = 1;

pub struct RestoreResult {
    pub operation_id: u64,
    pub result: Result<RestoredAccount, RestoreFailure>,
}

#[derive(Clone, Copy, Debug)]
pub enum DeliveryFailure {
    Closed,
    Full,
    Wake,
}

#[derive(Debug)]
pub enum RuntimeFailure {
    Startup(io::Error),
    Busy,
    Closed,
    Task,
    Thread,
    Delivery(DeliveryFailure),
}

impl fmt::Display for RuntimeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Startup(error) => write!(f, "无法启动认证后台：{error}"),
            Self::Busy => f.write_str("认证后台命令队列已满"),
            Self::Closed => f.write_str("认证后台已停止接收命令"),
            Self::Task => f.write_str("认证文件读取任务异常结束"),
            Self::Thread => f.write_str("认证运行线程异常结束"),
            Self::Delivery(DeliveryFailure::Closed) => f.write_str("认证结果接收端已关闭"),
            Self::Delivery(DeliveryFailure::Full) => f.write_str("认证结果队列已满"),
            Self::Delivery(DeliveryFailure::Wake) => {
                f.write_str("认证结果已入队，但无法唤醒应用窗口")
            }
        }
    }
}

impl Error for RuntimeFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Startup(error) => Some(error),
            _ => None,
        }
    }
}

pub struct AgentRuntime {
    commands: Option<mpsc::Sender<u64>>,
    thread: Option<JoinHandle<Result<(), RuntimeFailure>>>,
}

impl AgentRuntime {
    pub fn start(
        credentials_path: PathBuf,
        publish: impl Fn(RestoreResult) -> Result<(), DeliveryFailure> + Send + 'static,
        finished: impl Fn() -> Result<(), DeliveryFailure> + Send + 'static,
    ) -> Result<Self, RuntimeFailure> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .map_err(RuntimeFailure::Startup)?;
        let (commands, mut incoming) = mpsc::channel(COMMAND_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("agentelier-auth".into())
            .spawn(move || {
                debug!("认证运行线程已启动");
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async move {
                        let mut file = AuthFile::new(credentials_path);
                        while let Some(operation_id) = incoming.recv().await {
                            debug!(operation_id, "开始恢复当前目录账号");
                            let (returned, result) = tokio::task::spawn_blocking(move || {
                                let result = file.restore();
                                (file, result)
                            })
                            .await
                            .map_err(|_| RuntimeFailure::Task)?;
                            file = returned;
                            publish(RestoreResult {
                                operation_id,
                                result,
                            })
                            .map_err(RuntimeFailure::Delivery)?;
                            debug!(operation_id, "账号恢复结果已交付");
                        }
                        Ok(())
                    })
                }))
                .unwrap_or(Err(RuntimeFailure::Thread));
                // Dropping Tokio waits for its blocking file work before reporting termination.
                drop(runtime);
                debug!("认证执行体与 Tokio 已结束");
                let notification = finished().map_err(RuntimeFailure::Delivery);
                outcome.and(notification)
            })
            .map_err(RuntimeFailure::Startup)?;
        Ok(Self {
            commands: Some(commands),
            thread: Some(thread),
        })
    }

    pub fn restore(&self, operation_id: u64) -> Result<(), RuntimeFailure> {
        self.commands
            .as_ref()
            .ok_or(RuntimeFailure::Closed)?
            .try_send(operation_id)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => RuntimeFailure::Busy,
                mpsc::error::TrySendError::Closed(_) => RuntimeFailure::Closed,
            })
    }

    pub fn begin_shutdown(&mut self) {
        self.commands = None;
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn finish(&mut self) -> Result<(), RuntimeFailure> {
        self.begin_shutdown();
        match self.thread.take() {
            Some(thread) => thread.join().map_err(|_| RuntimeFailure::Thread)?,
            None => Ok(()),
        }
    }
}

impl Drop for AgentRuntime {
    fn drop(&mut self) {
        if let Err(failure) = self.finish() {
            error!(error = %failure, "未显式收尾的认证运行在释放时结束失败");
        }
    }
}
