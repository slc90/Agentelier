use agentelier_agent::{AgentEvent, AgentEventKind, AgentRuntime, DeliveryFailure};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::{error::Error, fmt};

const RESULT_CAPACITY: usize = 8;
type Notify = Box<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone, Debug)]
pub struct AccountSummary {
    pub subject: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
}
#[derive(Clone, Copy, Debug)]
pub enum CredentialState {
    NoRecord,
    HostOnly,
    SignedOut,
    StoredSession {
        expired: bool,
        subscription_enabled: bool,
    },
}
#[derive(Debug)]
pub struct AccountSnapshot {
    pub account: Option<AccountSummary>,
    pub credentials: CredentialState,
}
pub struct AuthorizationLink(String);
impl AuthorizationLink {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
pub struct AccountEvent {
    pub operation_id: u64,
    pub kind: AccountEventKind,
}
pub enum AccountEventKind {
    BrowserReady(AuthorizationLink),
    Exchanging,
    Saving,
    SaveFailed(BridgeFailure),
    Completed(Result<AccountSnapshot, BridgeFailure>),
    Cancelled,
}
#[derive(Clone, Debug)]
pub struct BridgeFailure(String);
impl fmt::Display for BridgeFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for BridgeFailure {}

struct WakeGate(Mutex<Option<Notify>>);
impl WakeGate {
    fn signal(&self) -> Result<(), DeliveryFailure> {
        let notify = self.0.lock().map_err(|_| DeliveryFailure::Wake)?;
        if notify.as_ref().is_some_and(|notify| !notify()) {
            return Err(DeliveryFailure::Wake);
        }
        Ok(())
    }
    fn disable(&self) -> Result<(), BridgeFailure> {
        self.0
            .lock()
            .map_err(|_| BridgeFailure("认证唤醒状态锁已损坏".into()))?
            .take();
        Ok(())
    }
}
#[derive(Clone, Copy)]
enum Phase {
    Restore,
    Login,
    SaveFailed,
    Saving,
}
struct Pending {
    id: u64,
    phase: Phase,
    cancelling: bool,
}

pub struct Connection {
    runtime: AgentRuntime,
    events: mpsc::Receiver<AccountEvent>,
    wake: Arc<WakeGate>,
    next_operation: u64,
    pending: Option<Pending>,
    stopping: bool,
    failure: Option<BridgeFailure>,
}
impl Connection {
    pub fn start(
        path: PathBuf,
        notify: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<Self, BridgeFailure> {
        let (sender, events) = mpsc::sync_channel(RESULT_CAPACITY);
        let wake = Arc::new(WakeGate(Mutex::new(Some(Box::new(notify)))));
        let delivery_wake = wake.clone();
        let finished_wake = wake.clone();
        let runtime = AgentRuntime::start(
            path,
            move |event| {
                sender
                    .try_send(convert(event))
                    .map_err(|error| match error {
                        mpsc::TrySendError::Full(_) => DeliveryFailure::Full,
                        mpsc::TrySendError::Disconnected(_) => DeliveryFailure::Closed,
                    })?;
                delivery_wake.signal()
            },
            move || finished_wake.signal(),
        )
        .map_err(|error| BridgeFailure(error.to_string()))?;
        Ok(Self {
            runtime,
            events,
            wake,
            next_operation: 0,
            pending: None,
            stopping: false,
            failure: None,
        })
    }
    fn allocate(&self) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("认证运行已停止".into()));
        }
        if self.pending.is_some() {
            return Err(BridgeFailure("已有账号操作正在进行".into()));
        }
        self.next_operation
            .checked_add(1)
            .ok_or_else(|| BridgeFailure("账号操作标识已耗尽".into()))
    }
    pub fn restore(&mut self) -> Result<u64, BridgeFailure> {
        let id = self.allocate()?;
        self.runtime
            .restore(id)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.next_operation = id;
        self.pending = Some(Pending {
            id,
            phase: Phase::Restore,
            cancelling: false,
        });
        Ok(id)
    }
    pub fn login(&mut self, consent: bool) -> Result<u64, BridgeFailure> {
        let id = self.allocate()?;
        self.runtime
            .login(id, consent)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.next_operation = id;
        self.pending = Some(Pending {
            id,
            phase: Phase::Login,
            cancelling: false,
        });
        Ok(id)
    }
    pub fn retry_save(&mut self, id: u64) -> Result<(), BridgeFailure> {
        if self.stopping
            || !self.pending.as_ref().is_some_and(|pending| {
                pending.id == id
                    && matches!(pending.phase, Phase::SaveFailed)
                    && !pending.cancelling
            })
        {
            return Err(BridgeFailure("当前操作没有可重试的保存结果".into()));
        }
        self.runtime
            .retry_save(id)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        if let Some(pending) = &mut self.pending {
            pending.phase = Phase::Saving;
        }
        Ok(())
    }
    pub fn cancel(&mut self, id: u64) -> Result<(), BridgeFailure> {
        if self.stopping {
            return Err(BridgeFailure("认证运行正在结束".into()));
        }
        let pending = self
            .pending
            .as_mut()
            .filter(|pending| pending.id == id)
            .ok_or_else(|| BridgeFailure("登录操作已经结束".into()))?;
        if matches!(pending.phase, Phase::Restore) {
            return Err(BridgeFailure("账号读取不支持中途取消".into()));
        }
        if !pending.cancelling {
            self.runtime
                .cancel(id)
                .map_err(|error| BridgeFailure(error.to_string()))?;
            pending.cancelling = true;
        }
        Ok(())
    }
    pub fn try_receive(&mut self) -> Result<Option<AccountEvent>, BridgeFailure> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        if !self.stopping && self.runtime.is_finished() {
            let failure = match self.runtime.finish() {
                Err(error) => BridgeFailure(error.to_string()),
                Ok(()) => BridgeFailure("认证运行意外停止".into()),
            };
            self.failure = Some(failure.clone());
            return Err(failure);
        }
        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    let Some(pending) = &mut self.pending else {
                        return self.protocol_failure();
                    };
                    if event.operation_id != pending.id {
                        return self.protocol_failure();
                    }
                    match &event.kind {
                        AccountEventKind::BrowserReady(_) if pending.cancelling => continue,
                        AccountEventKind::SaveFailed(_) => pending.phase = Phase::SaveFailed,
                        AccountEventKind::Saving => pending.phase = Phase::Saving,
                        AccountEventKind::Completed(_) | AccountEventKind::Cancelled => {
                            self.pending = None
                        }
                        _ => {}
                    }
                    return Ok(Some(event));
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(None),
                Err(mpsc::TryRecvError::Disconnected) if self.stopping => return Ok(None),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let failure = BridgeFailure("认证结果通道已关闭".into());
                    self.failure = Some(failure.clone());
                    return Err(failure);
                }
            }
        }
    }
    fn protocol_failure(&mut self) -> Result<Option<AccountEvent>, BridgeFailure> {
        let failure = BridgeFailure("账号结果不属于当前操作".into());
        self.failure = Some(failure.clone());
        Err(failure)
    }
    pub fn begin_shutdown(&mut self) -> Result<(), BridgeFailure> {
        self.stopping = true;
        let result = self.wake.disable();
        self.runtime.begin_shutdown();
        result
    }
    pub fn is_finished(&self) -> bool {
        self.runtime.is_finished()
    }
    pub fn finish(&mut self) -> Result<(), BridgeFailure> {
        let stop = self.begin_shutdown();
        let finish = self
            .runtime
            .finish()
            .map_err(|error| BridgeFailure(error.to_string()));
        stop?;
        finish?;
        if let Some(failure) = self.failure.take() {
            return Err(failure);
        }
        Ok(())
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Err(failure) = self.finish() {
            tracing::error!(error = %failure, "未显式收尾的业务桥接在释放时结束失败");
        }
    }
}
fn convert(event: AgentEvent) -> AccountEvent {
    let kind = match event.kind {
        AgentEventKind::BrowserReady(url) => AccountEventKind::BrowserReady(AuthorizationLink(url)),
        AgentEventKind::Exchanging => AccountEventKind::Exchanging,
        AgentEventKind::Saving => AccountEventKind::Saving,
        AgentEventKind::SaveFailed(failure) => {
            AccountEventKind::SaveFailed(BridgeFailure(failure.to_string()))
        }
        AgentEventKind::Cancelled => AccountEventKind::Cancelled,
        AgentEventKind::Completed(result) => AccountEventKind::Completed(
            result
                .map(|restored| {
                    let account = restored.account.map(|account| AccountSummary {
                        subject: account.subject,
                        display_name: account.display_name,
                        email: account.email,
                    });
                    let credentials = match restored.credentials {
                        agentelier_agent::CredentialState::NoRecord => CredentialState::NoRecord,
                        agentelier_agent::CredentialState::HostOnly => CredentialState::HostOnly,
                        agentelier_agent::CredentialState::SignedOut => CredentialState::SignedOut,
                        agentelier_agent::CredentialState::StoredSession {
                            expired,
                            subscription_enabled,
                        } => CredentialState::StoredSession {
                            expired,
                            subscription_enabled,
                        },
                    };
                    AccountSnapshot {
                        account,
                        credentials,
                    }
                })
                .map_err(|error| BridgeFailure(error.to_string())),
        ),
    };
    AccountEvent {
        operation_id: event.operation_id,
        kind,
    }
}
