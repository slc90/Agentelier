use serde_json::Value;
use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const REQUEST_CAPACITY: usize = 32;
const CANCELLATION_POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy)]
pub enum EcsOperation {
    WorldInfo,
}

#[derive(Clone, Debug)]
pub struct EcsError {
    pub code: &'static str,
    pub message: String,
}

impl EcsError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for EcsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for EcsError {}

#[derive(Clone)]
pub struct EcsAccess(SyncSender<EcsRequest>);

pub struct EcsRequest {
    pub operation: EcsOperation,
    cancel: CancellationToken,
    deadline: Instant,
    reply: SyncSender<Result<Value, EcsError>>,
}

impl EcsRequest {
    pub fn check(&self) -> Result<(), EcsError> {
        check(&self.cancel, self.deadline)
    }

    pub fn respond(self, result: Result<Value, EcsError>) {
        drop(self.reply.try_send(result));
    }
}

impl EcsAccess {
    pub fn channel() -> (Self, Receiver<EcsRequest>) {
        let (sender, receiver) = mpsc::sync_channel(REQUEST_CAPACITY);
        (Self(sender), receiver)
    }

    pub(crate) fn call(
        &self,
        operation: EcsOperation,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, EcsError> {
        check(cancel, deadline)?;
        let (reply, receiver) = mpsc::sync_channel(1);
        self.0
            .try_send(EcsRequest {
                operation,
                cancel: cancel.clone(),
                deadline,
                reply,
            })
            .map_err(|failure| match failure {
                TrySendError::Full(_) => EcsError::new("ecs_queue_full", "ECS 请求队列已满"),
                TrySendError::Disconnected(_) => closed(),
            })?;
        loop {
            check(cancel, deadline)?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining.min(CANCELLATION_POLL)) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err(closed()),
            }
        }
    }
}

fn check(cancel: &CancellationToken, deadline: Instant) -> Result<(), EcsError> {
    if cancel.is_cancelled() {
        Err(EcsError::new("cancelled", "ECS 脚本已取消"))
    } else if Instant::now() >= deadline {
        Err(EcsError::new("timeout", "ECS 脚本超过执行期限"))
    } else {
        Ok(())
    }
}

fn closed() -> EcsError {
    EcsError::new("ecs_host_closed", "ECS 宿主连接已关闭")
}
