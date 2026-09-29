//! One GUI's mutation gate. The OS instance lease covers other processes.
use rela_protocol::AppError;
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

const OPEN: u8 = 0;
const STARTING: u8 = 1;
const COMPLETING: u8 = 2;
const UPDATING: u8 = 3;
const FAILED: u8 = 4;

pub struct Gate {
    state: AtomicU8,
    operations: Arc<RwLock<()>>,
}
impl Gate {
    pub fn new(starting: bool) -> Self {
        Self {
            state: AtomicU8::new(if starting { STARTING } else { OPEN }),
            operations: Arc::new(RwLock::new(())),
        }
    }
    pub fn is_open(&self) -> bool {
        self.state.load(Ordering::Acquire) == OPEN
    }
    pub fn is_starting(&self) -> bool {
        matches!(self.state.load(Ordering::Acquire), STARTING | COMPLETING)
    }
    pub fn operation(&self) -> Result<OwnedRwLockReadGuard<()>, AppError> {
        let permit = self
            .operations
            .clone()
            .try_read_owned()
            .map_err(|_| busy())?;
        if !self.is_open() {
            return Err(busy());
        }
        Ok(permit)
    }
    pub fn begin_update(&self) -> Result<OwnedRwLockWriteGuard<()>, AppError> {
        let permit = self
            .operations
            .clone()
            .try_write_owned()
            .map_err(|_| busy())?;
        self.state
            .compare_exchange(OPEN, UPDATING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| busy())?;
        Ok(permit)
    }
    pub fn update_failed(&self) {
        let _ = self
            .state
            .compare_exchange(UPDATING, OPEN, Ordering::AcqRel, Ordering::Acquire);
    }
    /// React StrictMode may request completion twice; only the first owns IPC.
    pub fn begin_completion(&self) -> bool {
        self.state
            .compare_exchange(STARTING, COMPLETING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub fn completed(&self) {
        self.state.store(OPEN, Ordering::Release);
    }
    pub fn failed(&self) {
        self.state.store(FAILED, Ordering::Release);
    }
}
fn busy() -> AppError {
    AppError::new(
        "update_in_progress",
        "正在准备或完成软件更新，请稍候再操作。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_waits_for_existing_writers_and_blocks_new_writers() {
        let gate = Gate::new(false);
        let operation = gate.operation().unwrap();
        assert!(gate.begin_update().is_err());
        drop(operation);
        let update = gate.begin_update().unwrap();
        assert!(gate.operation().is_err());
        assert!(gate.begin_update().is_err());
        gate.update_failed();
        assert!(gate.operation().is_err());
        drop(update);
        assert!(gate.operation().is_ok());
    }
    #[test]
    fn candidate_stays_gated_until_commit_and_completion_is_owned_once() {
        let gate = Gate::new(true);
        assert!(gate.operation().is_err());
        assert!(gate.begin_completion());
        assert!(!gate.begin_completion());
        assert!(gate.operation().is_err());
        gate.completed();
        assert!(gate.operation().is_ok());
        gate.failed();
        assert!(gate.operation().is_err());
        assert!(!gate.begin_completion());
    }
}
