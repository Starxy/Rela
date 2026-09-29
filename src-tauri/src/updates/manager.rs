use rela_protocol::{AppError, UpdatePhase, UpdateProgress};
use std::sync::Mutex;

#[derive(Default)]
pub struct UpdateManager {
    progress: Mutex<UpdateProgress>,
}
impl UpdateManager {
    pub fn status(&self) -> Result<UpdateProgress, AppError> {
        self.progress
            .lock()
            .map(|value| value.clone())
            .map_err(|_| error())
    }
    pub fn begin(&self, version: String, total: u64) -> Result<(), AppError> {
        *self.progress.lock().map_err(|_| error())? = UpdateProgress {
            phase: UpdatePhase::Downloading,
            version: Some(version),
            downloaded: 0,
            total,
            error: None,
        };
        Ok(())
    }
    pub fn downloaded(&self, downloaded: u64, total: u64) {
        if let Ok(mut state) = self.progress.lock() {
            state.downloaded = downloaded.min(total);
            state.total = total;
            if downloaded == total {
                state.phase = UpdatePhase::Preparing;
            }
        }
    }
    pub fn restarting(&self) {
        if let Ok(mut state) = self.progress.lock() {
            state.phase = UpdatePhase::Restarting;
        }
    }
    pub fn failed(&self, failure: &AppError) {
        if let Ok(mut state) = self.progress.lock() {
            state.phase = UpdatePhase::Failed;
            state.error = Some(failure.message.clone());
        }
    }
}
fn error() -> AppError {
    AppError::new("update_status_failed", "无法读取软件更新进度。")
}
