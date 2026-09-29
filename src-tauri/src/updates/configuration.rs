//! Initiating-user configuration snapshots for installed application updates.
//! This code stays in the non-elevated controller. It never resolves AppData for
//! the administrator accepting UAC, and never follows a path from its journal.
use super::{
    portable::{
        copy_inventory, exists, inventory, plain, read, remove_file, remove_inventory_snapshot,
        validate_id, validate_inventory, Inventory,
    },
    startup::InstanceLease,
};
use crate::platform;
use rela_protocol::AppError;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

const JOURNAL: &str = "configuration.dat";
const MAX_RECORD: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    Restoring,
    RolledBack,
    Committed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    RestoreDecision,
    Displaced,
    Copied,
    Restored,
    RolledBack,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    id: String,
    config: PathBuf,
    phase: Phase,
    present: bool,
    before: Inventory,
    displaced: Option<Inventory>,
    cleaned: bool,
}

pub struct Transaction {
    config: PathBuf,
    session: PathBuf,
    journal: Journal,
}

impl Transaction {
    /// The exact old GUI must have exited; the lease excludes all other writers
    /// using this configuration. The caller supplies AppPaths::config directly.
    pub fn prepare(config: &Path, id: &str, lease: &InstanceLease) -> Result<Self, AppError> {
        validate_id(id)?;
        lease.protects(config)?;
        let (config, session) = locations(config)?;
        if exists(&session)? || exists(&archive_path(&session, id))? {
            return Err(error());
        }
        let present = exists(&config)?;
        let before = if present {
            inventory(&config)?
        } else {
            Inventory::default()
        };
        // Backup + restore staging must both fit, on the configuration's volume.
        let needed =
            32 * 1024 * 1024 + 2 * before.files.values().map(|file| file.size).sum::<u64>();
        if fs2::available_space(config.parent().ok_or_else(error)?).map_err(|_| error())? < needed {
            return Err(AppError::new(
                "update_space_insufficient",
                "配置备份空间不足，原配置未更改。",
            ));
        }
        platform::create_private_directory(&session)?;
        let result = Self {
            config: config.clone(),
            session,
            journal: Journal {
                schema_version: 1,
                id: id.into(),
                config,
                phase: Phase::Prepared,
                present,
                before,
                displaced: None,
                cleaned: false,
            },
        };
        if present {
            copy_inventory(
                &result.config,
                &result.session.join("before"),
                &result.journal.before,
            )?;
        }
        result.verify_backup()?;
        result.verify_old()?;
        result.save()?;
        Ok(result)
    }

    pub fn reopen(config: &Path, id: &str) -> Result<Self, AppError> {
        validate_id(id)?;
        let (config, mut session) = locations(config)?;
        if !exists(&session)? {
            session = archive_path(&session, id);
        }
        let journal: Journal = serde_json::from_slice(&platform::unprotect(&read(
            &session.join(JOURNAL),
            MAX_RECORD,
        )?)?)
        .map_err(|_| error())?;
        if journal.schema_version != 1
            || journal.id != id
            || journal.config != config
            || (!journal.present && journal.before != Inventory::default())
            || (matches!(journal.phase, Phase::Prepared | Phase::Committed)
                && journal.displaced.is_some())
            || (journal.cleaned && !matches!(journal.phase, Phase::Committed | Phase::RolledBack))
        {
            return Err(error());
        }
        validate_inventory(&journal.before)?;
        if let Some(displaced) = &journal.displaced {
            validate_inventory(displaced)?;
        }
        let result = Self {
            config,
            session,
            journal,
        };
        if !matches!(result.phase(), Phase::Committed | Phase::RolledBack) {
            result.verify_backup()?;
        }
        Ok(result)
    }

    pub fn phase(&self) -> Phase {
        self.journal.phase
    }
    pub fn directory(&self) -> &Path {
        &self.session
    }
    fn save(&self) -> Result<(), AppError> {
        plain(&self.session, true)?;
        let bytes = platform::protect(&serde_json::to_vec(&self.journal).map_err(|_| error())?)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(error());
        }
        let temporary = self.session.join("configuration.pending");
        remove_file(&temporary)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| error())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| error())?;
        drop(file);
        platform::replace_file(&temporary, &self.session.join(JOURNAL))
    }
    fn set_phase(&mut self, phase: Phase) -> Result<(), AppError> {
        let previous = self.journal.phase;
        self.journal.phase = phase;
        if let Err(error) = self.save() {
            self.journal.phase = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Called ONLY after the protected installed-body decision is Committing or
    /// Committed. Candidate configuration remains behind its startup write gate.
    pub fn commit(&mut self) -> Result<(), AppError> {
        if self.phase() == Phase::Committed {
            return Ok(());
        }
        if self.phase() != Phase::Prepared {
            return Err(error());
        }
        self.verify_backup()?;
        if exists(&self.config)? {
            inventory(&self.config)?;
        }
        self.set_phase(Phase::Committed)
    }

    /// Called only after the worker confirms no durable body commit, and after
    /// this update's candidate has exited. Never infer rollback from a timeout.
    pub fn rollback(&mut self, lease: &InstanceLease) -> Result<(), AppError> {
        self.rollback_observed(lease, |_| Ok(()))
    }
    fn rollback_observed(
        &mut self,
        lease: &InstanceLease,
        mut observe: impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        lease.protects(&self.config)?;
        if self.phase() == Phase::RolledBack {
            return self.verify_old();
        }
        if !matches!(self.phase(), Phase::Prepared | Phase::Restoring) {
            return Err(error());
        }
        self.verify_backup()?;
        if self.phase() == Phase::Prepared {
            self.journal.displaced = if exists(&self.config)? {
                Some(inventory(&self.config)?)
            } else {
                None
            };
            if let Err(error) = self.set_phase(Phase::Restoring) {
                self.journal.displaced = None;
                return Err(error);
            }
        }
        observe(Step::RestoreDecision)?;
        let failed = self.session.join("failed");
        if exists(&failed)? {
            if Some(inventory(&failed)?) != self.journal.displaced {
                return Err(error());
            }
            if exists(&self.config)? {
                self.verify_old()?;
                self.set_phase(Phase::RolledBack)?;
                return observe(Step::RolledBack);
            }
        } else if let Some(expected) = &self.journal.displaced {
            if !exists(&self.config)? || inventory(&self.config)? != *expected {
                return Err(error());
            }
            platform::move_file_new(&self.config, &failed)?;
        } else if exists(&self.config)? {
            // No candidate config existed; a previous attempt may have already
            // published the old snapshot. Never overwrite an unrelated new tree.
            self.verify_old()?;
            self.set_phase(Phase::RolledBack)?;
            return observe(Step::RolledBack);
        }
        observe(Step::Displaced)?;
        if self.journal.present {
            let restore = self.session.join("restore");
            copy_inventory(&self.session.join("before"), &restore, &self.journal.before)?;
            observe(Step::Copied)?;
            platform::move_file_new(&restore, &self.config)?;
        }
        observe(Step::Restored)?;
        self.verify_old()?;
        self.set_phase(Phase::RolledBack)?;
        observe(Step::RolledBack)
    }

    /// Removes only authenticated snapshot content. Keep the sealed terminal
    /// record and failed candidate configuration for archive/acknowledgement loss.
    pub fn cleanup(&mut self) -> Result<(), AppError> {
        if !matches!(self.phase(), Phase::Committed | Phase::RolledBack) {
            return Err(error());
        }
        if self.phase() == Phase::RolledBack {
            self.verify_old()?;
        }
        self.validate_contents()?;
        for name in ["before", "restore"] {
            remove_inventory_snapshot(&self.session.join(name), &self.journal.before)?;
        }
        remove_file(&self.session.join("configuration.pending"))?;
        let previous = self.journal.cleaned;
        self.journal.cleaned = true;
        if let Err(error) = self.save() {
            self.journal.cleaned = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Retain the terminal receipt and any failed migration data. The controller
    /// may repeat this after its final acknowledgement was lost.
    pub fn archive(&mut self) -> Result<PathBuf, AppError> {
        if !self.journal.cleaned || !matches!(self.phase(), Phase::Committed | Phase::RolledBack) {
            return Err(error());
        }
        self.validate_contents()?;
        let (_, active) = locations(&self.config)?;
        let target = archive_path(&active, &self.journal.id);
        if self.session != target {
            platform::move_file_new(&self.session, &target)?;
            self.session = target.clone();
        }
        Ok(target)
    }

    fn validate_contents(&self) -> Result<(), AppError> {
        for entry in fs::read_dir(&self.session).map_err(|_| error())? {
            let entry = entry.map_err(|_| error())?;
            let name = entry.file_name().into_string().map_err(|_| error())?;
            if !matches!(
                name.as_str(),
                JOURNAL | "configuration.pending" | "before" | "restore" | "failed"
            ) {
                return Err(error());
            }
            plain(
                &entry.path(),
                !matches!(name.as_str(), JOURNAL | "configuration.pending"),
            )?;
        }
        let failed = self.session.join("failed");
        if exists(&failed)? && Some(inventory(&failed)?) != self.journal.displaced {
            return Err(error());
        }
        if self.phase() == Phase::RolledBack
            && self.journal.displaced.is_some()
            && !exists(&failed)?
        {
            return Err(error());
        }
        Ok(())
    }
    fn verify_backup(&self) -> Result<(), AppError> {
        let before = self.session.join("before");
        if exists(&before)? != self.journal.present
            || (self.journal.present && inventory(&before)? != self.journal.before)
        {
            return Err(error());
        }
        Ok(())
    }
    fn verify_old(&self) -> Result<(), AppError> {
        if exists(&self.config)? != self.journal.present
            || (self.journal.present && inventory(&self.config)? != self.journal.before)
        {
            return Err(error());
        }
        Ok(())
    }
}

fn locations(config: &Path) -> Result<(PathBuf, PathBuf), AppError> {
    let parent = config.parent().ok_or_else(error)?;
    plain(parent, true)?;
    let parent = parent.canonicalize().map_err(|_| error())?;
    let config = parent.join(config.file_name().ok_or_else(error)?);
    if exists(&config)? {
        plain(&config, true)?;
    }
    let key = rela_manifests::sha256(config.to_string_lossy().to_lowercase().as_bytes());
    Ok((config, parent.join(format!(".rela-configuration-{key}"))))
}

/// Only the controller's persisted ConfigPreparing state authorizes this.
/// No candidate or installer has started, so live configuration is not changed.
pub(crate) fn retain_incomplete(
    config: &Path,
    id: &str,
    lease: &InstanceLease,
) -> Result<(), AppError> {
    validate_id(id)?;
    lease.protects(config)?;
    let (_, session) = locations(config)?;
    if exists(&archive_path(&session, id))? {
        return Err(error());
    }
    if !exists(&session)? {
        return Ok(());
    }
    plain(&session, true)?;
    if exists(&session.join(JOURNAL))? {
        return Err(error());
    }
    let target = session.with_file_name(format!(
        ".rela-configuration-preparation-{id}-{}",
        crate::updates::ipc::random_id()?
    ));
    platform::move_file_new(&session, &target)
}
fn archive_path(session: &Path, id: &str) -> PathBuf {
    session.with_file_name(format!(
        "{}-finished-{id}",
        session.file_name().unwrap_or_default().to_string_lossy()
    ))
}
fn error() -> AppError {
    AppError::new(
        "update_configuration_recovery_required",
        "更新配置恢复尚未完成。原配置备份已保留，请重试恢复。",
    )
}

#[cfg(test)]
mod tests;
