use crate::runtime::{Command, RuntimeFailure};
use agentelier_login::{
    AuthenticatedRequest, CredentialVersion, SessionFailure, SessionFailureKind, UsableCredentials,
};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

type Outcome = Result<UsableCredentials, SessionFailure>;

#[derive(Clone, PartialEq, Eq)]
pub(super) enum CredentialIntent {
    Acquire,
    Recover(CredentialVersion),
}

struct Flight {
    intent: CredentialIntent,
    result: watch::Receiver<Option<Outcome>>,
}
struct ClientState {
    flights: Vec<Flight>,
    accepting: bool,
    ending: bool,
    invalidated: CancellationToken,
}

#[derive(Clone)]
pub struct CredentialClient {
    commands: mpsc::Sender<Command>,
    state: Arc<Mutex<ClientState>>,
    shutdown: CancellationToken,
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
                return Err(SessionFailure::new(
                    SessionFailureKind::SignedOut,
                    "该请求所属会话已结束，不能交付旧凭据",
                ));
            }
            if self.shutdown.is_cancelled() {
                return Err(SessionFailure::new(
                    SessionFailureKind::Runtime,
                    "认证运行正在结束",
                ));
            }
            if let Some(result) = self.result.borrow_and_update().clone() {
                return result;
            }
            tokio::select! { biased;
                _ = self.invalidated.cancelled() => continue,
                _ = self.shutdown.cancelled() => continue,
                result = self.result.changed() => result.map_err(|_| {
                SessionFailure::new(
                    SessionFailureKind::Runtime,
                    "认证运行已结束，未返回可用凭据",
                )
                })?,
            }
        }
    }
}
impl CredentialClient {
    pub(super) fn new(commands: mpsc::Sender<Command>, shutdown: CancellationToken) -> Self {
        Self {
            commands,
            state: Arc::new(Mutex::new(ClientState {
                flights: Vec::new(),
                accepting: true,
                ending: false,
                invalidated: CancellationToken::new(),
            })),
            shutdown,
        }
    }
    pub fn request(&self) -> Result<CredentialRequest, RuntimeFailure> {
        self.submit(CredentialIntent::Acquire)
    }
    pub fn recover(
        &self,
        request: &mut AuthenticatedRequest,
        status: u16,
    ) -> Result<Option<CredentialRequest>, RuntimeFailure> {
        request
            .begin_recovery(status)
            .map(|version| self.submit(CredentialIntent::Recover(version)))
            .transpose()
    }
    fn submit(&self, intent: CredentialIntent) -> Result<CredentialRequest, RuntimeFailure> {
        if self.shutdown.is_cancelled() {
            return Err(RuntimeFailure::Closed);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| RuntimeFailure::CredentialState)?;
        if !state.accepting {
            return Err(RuntimeFailure::SignedOut);
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
        self.commands
            .try_send(Command::Credentials {
                intent: intent.clone(),
                result,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => RuntimeFailure::Busy,
                mpsc::error::TrySendError::Closed(_) => RuntimeFailure::Closed,
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
    pub(super) fn submit_operation(
        &self,
        submit: impl FnOnce() -> Result<(), RuntimeFailure>,
    ) -> Result<(), RuntimeFailure> {
        let state = self
            .state
            .lock()
            .map_err(|_| RuntimeFailure::CredentialState)?;
        if state.ending {
            return Err(RuntimeFailure::SigningOut);
        }
        submit()
    }
    pub(super) fn begin_sign_out(
        &self,
        submit: impl FnOnce() -> Result<(), RuntimeFailure>,
    ) -> Result<(), RuntimeFailure> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RuntimeFailure::CredentialState)?;
        submit()?;
        state.accepting = false;
        state.ending = true;
        state.invalidated.cancel();
        state.flights.clear();
        Ok(())
    }
    pub(super) fn is_signing_out(&self) -> Result<bool, RuntimeFailure> {
        Ok(self
            .state
            .lock()
            .map_err(|_| RuntimeFailure::CredentialState)?
            .ending)
    }
    pub(super) fn finish_sign_out(&self) -> Result<(), RuntimeFailure> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RuntimeFailure::CredentialState)?;
        state.ending = false;
        state.accepting = false;
        Ok(())
    }
    pub(super) fn resume_after_login(&self) -> Result<(), RuntimeFailure> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| RuntimeFailure::CredentialState)?;
        if !state.ending && !state.accepting {
            state.invalidated = CancellationToken::new();
            state.accepting = true;
        }
        Ok(())
    }
}
