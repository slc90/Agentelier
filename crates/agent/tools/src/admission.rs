use std::sync::Arc;
use tokio::sync::{OwnedRwLockWriteGuard, OwnedSemaphorePermit, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

const MAX_RUNNING_TOOLS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolAccess {
    SharedRead,
    #[default]
    Exclusive,
    ProcessControl,
}

#[derive(Clone)]
pub struct ToolAdmission {
    effects: Arc<RwLock<()>>,
    slots: Arc<Semaphore>,
    blocked: CancellationToken,
}

impl Default for ToolAdmission {
    fn default() -> Self {
        Self {
            effects: Arc::new(RwLock::new(())),
            slots: Arc::new(Semaphore::new(MAX_RUNNING_TOOLS)),
            blocked: CancellationToken::new(),
        }
    }
}

pub struct ToolPermit {
    _slot: Option<OwnedSemaphorePermit>,
    _shared: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
    pub(crate) exclusive: Option<OwnedRwLockWriteGuard<()>>,
}

impl ToolAdmission {
    pub fn block(&self) {
        self.blocked.cancel();
    }

    pub fn is_blocked(&self) -> bool {
        self.blocked.is_cancelled()
    }

    pub async fn blocked(&self) {
        self.blocked.cancelled().await;
    }

    // The caller supplies one FIFO head for ordinary work. Process control has its own lane.
    pub async fn acquire(
        &self,
        access: ToolAccess,
        cancel: &CancellationToken,
    ) -> Option<ToolPermit> {
        if access == ToolAccess::ProcessControl {
            return (!cancel.is_cancelled()).then_some(ToolPermit {
                _slot: None,
                _shared: None,
                exclusive: None,
            });
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            _ = self.blocked.cancelled() => None,
            permit = self.acquire_ordinary(access) => permit,
        }
    }

    async fn acquire_ordinary(&self, access: ToolAccess) -> Option<ToolPermit> {
        loop {
            let (shared, exclusive) = if access == ToolAccess::SharedRead {
                (Some(self.effects.clone().read_owned().await), None)
            } else {
                (None, Some(self.effects.clone().write_owned().await))
            };
            if let Ok(slot) = self.slots.clone().try_acquire_owned() {
                return Some(ToolPermit {
                    _slot: Some(slot),
                    _shared: shared,
                    exclusive,
                });
            }
            drop((shared, exclusive));
            drop(self.slots.acquire().await.ok()?);
        }
    }
}
