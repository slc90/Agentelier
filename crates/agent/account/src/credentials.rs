use crate::operations::{AccountCommand, AccountFailure};
use agentelier_login::{
    AuthenticatedRequest, CredentialVersion, SessionFailure, UsableCredentials,
};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

type Outcome = Result<UsableCredentials, SessionFailure>;

#[derive(Clone, PartialEq, Eq)]
pub enum CredentialIntent {
    Acquire,
    Recover(CredentialVersion),
}

struct Flight {
    intent: CredentialIntent,
    result: watch::Receiver<Option<Outcome>>,
}
struct ClientState {
    flights: Vec<Flight>,
    codex_selected: bool,
    source_settling: bool,
    invalidated: CancellationToken,
}

#[derive(Clone)]
pub struct CredentialClient {
    submit: Arc<dyn Fn(AccountCommand) -> Result<(), AccountFailure> + Send + Sync>,
    state: Arc<Mutex<ClientState>>,
    shutdown: CancellationToken,
}

#[derive(Clone)]
pub struct ExecutionIdentity {
    version: CredentialVersion,
    invalidated: CancellationToken,
}
impl ExecutionIdentity {
    pub fn identity(&self) -> &crate::CodexIdentity {
        self.version.identity()
    }

    pub async fn invalidated(&self) {
        self.invalidated.cancelled().await;
    }

    pub fn validate(&self, credentials: &UsableCredentials) -> Result<(), SessionFailure> {
        if self.invalidated.is_cancelled() || self.identity() != credentials.version().identity() {
            return Err(SessionFailure::new(
                "本次任务的账号会话已结束，请使用当前账号开始新任务",
            ));
        }
        Ok(())
    }

    fn ensure_active(&self) -> Result<(), SessionFailure> {
        if self.invalidated.is_cancelled() {
            Err(SessionFailure::new("本次任务绑定的授权已失效"))
        } else {
            Ok(())
        }
    }
}

pub struct CredentialRequest {
    result: watch::Receiver<Option<Outcome>>,
    invalidated: CancellationToken,
    shutdown: CancellationToken,
}
impl CredentialRequest {
    pub async fn wait(mut self) -> Outcome {
        loop {
            if self.invalidated.is_cancelled() {
                return Err(SessionFailure::new("该请求所属会话已结束，不能交付旧凭据"));
            }
            if self.shutdown.is_cancelled() {
                return Err(SessionFailure::new("认证运行正在结束"));
            }
            if let Some(result) = self.result.borrow_and_update().clone() {
                return result;
            }
            tokio::select! { biased;
                _ = self.invalidated.cancelled() => continue,
                _ = self.shutdown.cancelled() => continue,
                result = self.result.changed() => result.map_err(|_| {
                SessionFailure::new(
                    "认证运行已结束，未返回可用凭据",
                )
                })?,
            }
        }
    }
}
impl CredentialClient {
    pub fn new(
        shutdown: CancellationToken,
        codex_selected: bool,
        submit: impl Fn(AccountCommand) -> Result<(), AccountFailure> + Send + Sync + 'static,
    ) -> Self {
        Self {
            submit: Arc::new(submit),
            state: Arc::new(Mutex::new(ClientState {
                flights: Vec::new(),
                codex_selected,
                source_settling: false,
                invalidated: CancellationToken::new(),
            })),
            shutdown,
        }
    }
    pub async fn execution(&self) -> Result<ExecutionIdentity, SessionFailure> {
        let request = self
            .request()
            .map_err(|failure| SessionFailure::new(failure.to_string()))?;
        let invalidated = request.invalidated.clone();
        let credentials = request.wait().await?;
        let identity = ExecutionIdentity {
            version: credentials.version().clone(),
            invalidated,
        };
        identity.validate(&credentials)?;
        Ok(identity)
    }
    pub fn request(&self) -> Result<CredentialRequest, AccountFailure> {
        self.submit(CredentialIntent::Acquire)
    }

    pub async fn validate_execution(
        &self,
        identity: &ExecutionIdentity,
    ) -> Result<(), SessionFailure> {
        identity.ensure_active()?;
        let request = self
            .request()
            .map_err(|failure| SessionFailure::new(failure.to_string()))?;
        tokio::select! { biased;
            _ = identity.invalidated() => identity.ensure_active(),
            result = request.wait() => identity.validate(&result?),
        }
    }
    pub fn recover(
        &self,
        request: &mut AuthenticatedRequest,
        status: u16,
    ) -> Result<Option<CredentialRequest>, AccountFailure> {
        request
            .begin_recovery(status)
            .map(|version| self.submit(CredentialIntent::Recover(version)))
            .transpose()
    }
    fn submit(&self, intent: CredentialIntent) -> Result<CredentialRequest, AccountFailure> {
        if self.shutdown.is_cancelled() {
            return Err(AccountFailure::Closed);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| AccountFailure::CredentialState)?;
        if !state.codex_selected {
            return Err(AccountFailure::Disconnected);
        }
        if state.source_settling {
            return Err(AccountFailure::SourceSettling);
        }
        state.flights.retain(|flight| {
            flight.result.borrow().is_none() && flight.result.has_changed().is_ok()
        });
        if let Some(flight) = state.flights.iter().find(|flight| flight.intent == intent) {
            return Ok(CredentialRequest {
                result: flight.result.clone(),
                invalidated: state.invalidated.clone(),
                shutdown: self.shutdown.clone(),
            });
        }
        let (result, receiver) = watch::channel(None);
        (self.submit)(AccountCommand::Credentials {
            current: state.invalidated.clone(),
            intent: intent.clone(),
            result,
        })?;
        state.flights.push(Flight {
            intent,
            result: receiver.clone(),
        });
        Ok(CredentialRequest {
            result: receiver,
            invalidated: state.invalidated.clone(),
            shutdown: self.shutdown.clone(),
        })
    }
    pub fn submit_operation(
        &self,
        submit: impl FnOnce() -> Result<(), AccountFailure>,
    ) -> Result<(), AccountFailure> {
        let state = self
            .state
            .lock()
            .map_err(|_| AccountFailure::CredentialState)?;
        if state.source_settling {
            return Err(AccountFailure::SourceSettling);
        }
        submit()
    }
    pub(crate) fn update_source(
        &self,
        selected: bool,
        changed: bool,
    ) -> Result<(), AccountFailure> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AccountFailure::CredentialState)?;
        if changed || state.codex_selected != selected {
            state.invalidated.cancel();
            state.flights.clear();
            state.invalidated = CancellationToken::new();
            state.codex_selected = selected;
            state.source_settling = true;
        }
        Ok(())
    }

    pub fn source_settling(&self) -> Result<bool, AccountFailure> {
        Ok(self
            .state
            .lock()
            .map_err(|_| AccountFailure::CredentialState)?
            .source_settling)
    }

    pub fn finish_source_change(&self) -> Result<(), AccountFailure> {
        self.state
            .lock()
            .map_err(|_| AccountFailure::CredentialState)?
            .source_settling = false;
        Ok(())
    }
}
