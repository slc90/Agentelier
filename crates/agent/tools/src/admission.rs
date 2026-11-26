use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
use tokio_util::sync::CancellationToken;

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
    blocked: CancellationToken,
    paused: Arc<AtomicBool>,
    external_unconfirmed: Arc<AtomicBool>,
    external_waiting_save: Arc<AtomicUsize>,
}

impl Default for ToolAdmission {
    fn default() -> Self {
        Self {
            effects: Arc::new(RwLock::new(())),
            blocked: CancellationToken::new(),
            paused: Arc::default(),
            external_unconfirmed: Arc::default(),
            external_waiting_save: Arc::default(),
        }
    }
}

pub struct ToolPermit {
    _shared: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
    pub(crate) exclusive: Option<OwnedRwLockWriteGuard<()>>,
    external_waiting_save: Arc<AtomicUsize>,
}

impl ToolPermit {
    pub(crate) fn mark_external_unconfirmed(&self) {
        self.external_waiting_save.fetch_add(1, Ordering::AcqRel);
    }
}

impl ToolAdmission {
    pub fn pause(&self, value: bool) {
        self.paused.store(value, Ordering::Release);
    }

    pub fn external_unconfirmed(&self, value: bool) {
        self.external_unconfirmed.store(value, Ordering::Release);
    }

    pub fn external_result_saved(&self) {
        self.external_unconfirmed(true);
        let _updated =
            self.external_waiting_save
                .try_update(Ordering::AcqRel, Ordering::Acquire, |waiting| {
                    waiting.checked_sub(1)
                });
    }

    pub fn rejection(&self, access: ToolAccess) -> Option<&'static str> {
        if self.paused.load(Ordering::Acquire) {
            Some("background_paused")
        } else if access == ToolAccess::Exclusive
            && (self.external_waiting_save.load(Ordering::Acquire) != 0
                || self.external_unconfirmed.load(Ordering::Acquire))
        {
            Some("external_effects_unconfirmed")
        } else {
            None
        }
    }

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
                _shared: None,
                exclusive: None,
                external_waiting_save: self.external_waiting_save.clone(),
            });
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            _ = self.blocked.cancelled() => None,
            permit = self.acquire_ordinary(access) => Some(permit),
        }
    }

    async fn acquire_ordinary(&self, access: ToolAccess) -> ToolPermit {
        let (shared, exclusive) = if access == ToolAccess::SharedRead {
            (Some(self.effects.clone().read_owned().await), None)
        } else {
            (None, Some(self.effects.clone().write_owned().await))
        };
        ToolPermit {
            _shared: shared,
            exclusive,
            external_waiting_save: self.external_waiting_save.clone(),
        }
    }
}
