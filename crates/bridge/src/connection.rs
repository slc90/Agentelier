use agentelier_agent::{AgentRuntime, DeliveryFailure, RestoreResult};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::{error::Error, fmt};

const RESULT_CAPACITY: usize = 1;
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
    StoredSession { expired: bool },
}

#[derive(Debug)]
pub struct AccountEvent {
    pub operation_id: u64,
    pub result: Result<(Option<AccountSummary>, CredentialState), BridgeFailure>,
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

pub struct Connection {
    runtime: AgentRuntime,
    events: mpsc::Receiver<AccountEvent>,
    wake: Arc<WakeGate>,
    next_operation: u64,
    pending: Option<u64>,
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

    pub fn restore(&mut self) -> Result<u64, BridgeFailure> {
        if self.stopping || self.failure.is_some() {
            return Err(BridgeFailure("认证运行已停止".into()));
        }
        if self.pending.is_some() {
            return Err(BridgeFailure("账号恢复仍在进行".into()));
        }
        let id = self
            .next_operation
            .checked_add(1)
            .ok_or_else(|| BridgeFailure("账号操作标识已耗尽".into()))?;
        self.runtime
            .restore(id)
            .map_err(|error| BridgeFailure(error.to_string()))?;
        self.next_operation = id;
        self.pending = Some(id);
        Ok(id)
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
        match self.events.try_recv() {
            Ok(event) => {
                if Some(event.operation_id) != self.pending {
                    let failure = BridgeFailure("账号恢复结果不属于当前操作".into());
                    self.failure = Some(failure.clone());
                    return Err(failure);
                }
                self.pending = None;
                Ok(Some(event))
            }
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) if self.stopping => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                let failure = BridgeFailure("认证结果通道已关闭".into());
                self.failure = Some(failure.clone());
                Err(failure)
            }
        }
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

fn convert(event: RestoreResult) -> AccountEvent {
    let result = event
        .result
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
                agentelier_agent::CredentialState::StoredSession { expired } => {
                    CredentialState::StoredSession { expired }
                }
            };
            (account, credentials)
        })
        .map_err(|error| BridgeFailure(error.to_string()));
    AccountEvent {
        operation_id: event.operation_id,
        result,
    }
}
