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

#[derive(Clone)]
pub struct CredentialClient {
    commands: mpsc::Sender<Command>,
    flights: Arc<Mutex<Vec<Flight>>>,
    shutdown: CancellationToken,
}

pub struct CredentialRequest {
    result: watch::Receiver<Option<Outcome>>,
}
impl CredentialRequest {
    pub async fn wait(mut self) -> Outcome {
        loop {
            if let Some(result) = self.result.borrow_and_update().clone() {
                return result;
            }
            self.result.changed().await.map_err(|_| {
                SessionFailure::new(
                    SessionFailureKind::Runtime,
                    "认证运行已结束，未返回可用凭据",
                )
            })?;
        }
    }
}
impl CredentialClient {
    pub(super) fn new(commands: mpsc::Sender<Command>, shutdown: CancellationToken) -> Self {
        Self {
            commands,
            flights: Arc::new(Mutex::new(Vec::new())),
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
        let mut flights = self
            .flights
            .lock()
            .map_err(|_| RuntimeFailure::CredentialState)?;
        flights.retain(|flight| {
            flight.result.borrow().is_none() && flight.result.has_changed().is_ok()
        });
        if let Some(flight) = flights.iter().find(|flight| flight.intent == intent) {
            return Ok(CredentialRequest {
                result: flight.result.clone(),
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
        flights.push(Flight {
            intent,
            result: receiver.clone(),
        });
        Ok(CredentialRequest { result: receiver })
    }
}
