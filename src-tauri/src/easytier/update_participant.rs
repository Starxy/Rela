//! Elevated participant held by the application update helper. It never chooses
//! a commit decision from a timeout, and does not expose a Tauri command.
use super::{
    deployment::{Decision, Files},
    managed_service::ManagedService,
    verify_assets,
};
use crate::platform;
use rela_protocol::AppError;
use std::{fs::File, path::Path};

pub struct CoreUpdateParticipant {
    files: Files,
    service: ManagedService,
    id: String,
    active: bool,
    // The OS releases this on helper exit; the application journal remains pending.
    _control_lock: File,
}
impl CoreUpdateParticipant {
    pub fn prepare(bundled: &Path, id: &str) -> Result<Self, AppError> {
        let mut value = Self::open(id)?;
        value.prepare_update(bundled)?;
        Ok(value)
    }
    /// Keep the locked participant available for rollback even if preparation errors.
    pub fn prepare_update(&mut self, bundled: &Path) -> Result<bool, AppError> {
        self.active = true;
        verify_assets(bundled)?;
        self.active = self
            .files
            .prepare_application(bundled, &self.id, &mut self.service)?;
        Ok(self.active)
    }
    /// Reconnect to an existing application-owned journal without preparing again.
    pub fn resume(id: &str) -> Result<Self, AppError> {
        let mut value = Self::open(id)?;
        value.active = true;
        Ok(value)
    }
    fn open(id: &str) -> Result<Self, AppError> {
        super::deployment::validate_application_id(id)?;
        if !platform::is_elevated() {
            return Err(AppError::new(
                "elevation_required",
                "更新网络引擎需要管理员授权。",
            ));
        }
        let root = platform::service_directory()?;
        platform::secure_service_directory(&root)?;
        let lock = platform::lock_service_control(&root)?;
        Ok(Self {
            files: Files::new(
                root.clone(),
                platform::secure_service_directory,
                platform::secure_service_file,
            ),
            service: ManagedService::new(root),
            id: id.into(),
            active: false,
            _control_lock: lock,
        })
    }
    pub fn changed_service(&self) -> bool {
        self.active
    }
    pub fn commit(&mut self) -> Result<(), AppError> {
        if self.active {
            self.files
                .resolve_application(&self.id, Decision::Commit, &mut self.service)?;
        }
        Ok(())
    }
    pub fn rollback(&mut self) -> Result<(), AppError> {
        if self.active {
            self.files
                .resolve_application(&self.id, Decision::Rollback, &mut self.service)?;
        }
        Ok(())
    }
}
