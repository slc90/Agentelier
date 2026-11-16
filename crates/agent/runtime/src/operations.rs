use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Default)]
pub struct OperationSequence(Arc<AtomicU64>);

impl OperationSequence {
    pub fn allocate(&self) -> io::Result<u64> {
        self.0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map(|previous| previous + 1)
            .map_err(|_| io::Error::other("本次运行的操作身份已耗尽"))
    }
}
