use serde::Deserialize;
use serde_json::{Map, Value};
use std::fmt;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const REQUEST_CAPACITY: usize = 32;
const CANCELLATION_POLL: Duration = Duration::from_millis(10);

pub enum EcsOperation {
    WorldInfo,
    ReflectTypes(Page),
    ReflectType(String),
    Query(Query),
    Exists(EcsEntity),
    Components(EcsEntity),
    Get(EcsEntity, String),
    Parent(EcsEntity),
    Children(EcsEntity, Page),
    Resources(Page),
    ResourceGet(String),
    Spawn(Map<String, Value>),
    Insert(EcsEntity, String, Value),
    Patch(EcsEntity, String, Map<String, Value>),
    Remove(EcsEntity, String),
    Despawn(EcsEntity),
    Reparent(EcsEntity, EcsEntity),
    Unparent(EcsEntity),
    ResourceInsert(String, Value),
    ResourceSet(String, Value),
    ResourcePatch(String, Map<String, Value>),
    ResourceRemove(String),
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Page {
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Query {
    pub with: Vec<String>,
    pub without: Vec<String>,
    pub name: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
}

#[derive(Clone, Copy)]
pub struct EcsEntity(u64);

impl EcsEntity {
    pub fn from_bits(bits: u64) -> Self {
        Self(bits)
    }
    pub fn bits(self) -> u64 {
        self.0
    }

    pub fn parse(value: &str) -> Result<Self, EcsError> {
        value
            .strip_prefix("ecs:")
            .and_then(|value| value.parse().ok())
            .map(Self)
            .ok_or_else(|| EcsError::new("invalid_entity", "应使用 ECS 查询返回的实体引用"))
    }
}

impl fmt::Display for EcsEntity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ecs:{}", self.0)
    }
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
pub struct EcsAccess {
    requests: SyncSender<EcsRequest>,
    wake: Arc<dyn Fn() -> bool + Send + Sync>,
}

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
    pub fn channel(
        wake: impl Fn() -> bool + Send + Sync + 'static,
    ) -> (Self, Receiver<EcsRequest>) {
        let (requests, receiver) = mpsc::sync_channel(REQUEST_CAPACITY);
        (
            Self {
                requests,
                wake: Arc::new(wake),
            },
            receiver,
        )
    }

    pub(crate) fn call(
        &self,
        operation: EcsOperation,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, EcsError> {
        let cancel = cancel.child_token();
        let _cancel_on_return = cancel.clone().drop_guard();
        check(&cancel, deadline)?;
        let (reply, receiver) = mpsc::sync_channel(1);
        self.requests
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
        if !(self.wake)() {
            return Err(closed());
        }
        loop {
            check(&cancel, deadline)?;
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
