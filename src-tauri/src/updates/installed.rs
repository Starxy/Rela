//! Recoverable perMachine NSIS changes. This module never runs an installer.
//! An authenticated elevated participant must hold the machine update lock,
//! quiesce the original/candidate processes, and supply a reverified signed stage.
//! User configuration remains in its separate, non-elevated transaction.
pub mod guard;
pub mod metadata;
#[cfg(windows)]
pub mod native;

use super::portable::{
    copy_new, exists, fingerprint, fingerprint_optional, plain, read, remove_file,
    remove_program_snapshot, validate_fingerprint, validate_id, Fingerprint, MAX_PROGRAM,
};
use crate::{
    distribution::portable::{PortableIndex, PortableStage, UPDATE_FILES},
    platform,
};
use metadata::{Field, Shortcut, Snapshot, Value};
use rela_protocol::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub const DIRECTORY: &str = ".rela-installed-update";
const JOURNAL_LIMIT: u64 = 2 * 1024 * 1024;

/// The real implementation is restricted to the two Rela HKLM keys and two
/// all-users shortcut locations. Tests use memory records and temporary files.
pub trait SystemState {
    fn validate_root(&self, root: &Path) -> Result<(), AppError>;
    fn validate_live_file(&self, path: &Path) -> Result<(), AppError>;
    fn create_session(&self, path: &Path) -> Result<(), AppError>;
    fn validate_session(&self, path: &Path) -> Result<(), AppError>;
    fn snapshot(&self) -> Result<Snapshot, AppError>;
    fn restore_registry(&mut self, field: Field, value: Option<&Value>) -> Result<(), AppError>;
    fn restore_shortcut(&mut self, shortcut: Shortcut, value: Option<&str>)
        -> Result<(), AppError>;
    /// Files copied out of protected backups must become readable/executable by
    /// ordinary users again, without granting those users write/delete access.
    fn publish_restored_file(&self, path: &Path) -> Result<(), AppError>;
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    Installed,
    Committing,
    Committed,
    RolledBack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    RestoredFile(usize),
    RestoredRegistry(Field),
    RestoredShortcut(Shortcut),
    RolledBack,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    id: String,
    root: PathBuf,
    phase: Phase,
    previous_version: Version,
    next: PortableIndex,
    old_program: BTreeMap<String, Option<Fingerprint>>,
    before: Snapshot,
    after: Option<Snapshot>,
    new_uninstaller: Option<Fingerprint>,
}

pub struct Transaction {
    root: PathBuf,
    session: PathBuf,
    journal: Journal,
}

pub fn program_names() -> impl Iterator<Item = &'static str> {
    UPDATE_FILES
        .iter()
        .copied()
        .filter(|name| *name != "README.txt")
}
fn backup_names() -> impl Iterator<Item = &'static str> {
    program_names().chain(std::iter::once("uninstall.exe"))
}

impl Transaction {
    pub fn prepare(
        root: &Path,
        stage: &PortableStage,
        previous_version: Version,
        id: &str,
        system: &impl SystemState,
    ) -> Result<Self, AppError> {
        validate_id(id)?;
        plain(root, true)?;
        let root = root.canonicalize().map_err(|_| pending())?;
        system.validate_root(&root)?;
        if exists(&root.join("portable.txt"))?
            || exists(&receipt_path(&root, id))?
            || previous_version >= stage.index().version
            || !previous_version.build.is_empty()
        {
            return Err(pending());
        }
        let _stage_guard = stage.lock()?;
        let before = system.snapshot()?;
        before.validate_installation(&root, &previous_version)?;
        let session = root.join(DIRECTORY);
        if exists(&session)? {
            return Err(pending());
        }
        let mut needed = 32 * 1024 * 1024;
        let mut largest = 0;
        let mut old_program = BTreeMap::new();
        for name in backup_names() {
            let path = root.join(name);
            let old = fingerprint_optional(&path, MAX_PROGRAM)?;
            if old.is_none() && matches!(name, "Rela.exe" | "uninstall.exe") {
                return Err(pending());
            }
            if let Some(value) = &old {
                system.validate_live_file(&path)?;
                probe_replace(&path)?;
                needed += value.size;
                largest = largest.max(value.size);
            }
            old_program.insert(name.into(), old);
        }
        for name in program_names() {
            let size = fingerprint(&stage.directory().join(name), MAX_PROGRAM)?.size;
            needed += size;
            largest = largest.max(size);
        }
        if fs2::available_space(&root).map_err(|_| pending())? < needed + largest {
            return Err(AppError::new(
                "update_space_insufficient",
                "安装版更新备份空间不足，原程序未更改。",
            ));
        }
        system.create_session(&session)?;
        system.validate_session(&session)?;
        let result = Self {
            root: root.clone(),
            session,
            journal: Journal {
                schema_version: 1,
                id: id.into(),
                root,
                phase: Phase::Prepared,
                previous_version,
                next: stage.index().clone(),
                old_program,
                before,
                after: None,
                new_uninstaller: None,
            },
        };
        for directory in ["old", "next"] {
            let parent = result.session.join(directory);
            fs::create_dir(&parent).map_err(|_| pending())?;
            for child in ["easytier", "third-party-licenses"] {
                fs::create_dir(parent.join(child)).map_err(|_| pending())?;
            }
        }
        for (name, old) in &result.journal.old_program {
            if let Some(old) = old {
                copy_new(
                    &result.root.join(name),
                    &result.session.join("old").join(name),
                    old,
                )?;
            }
        }
        for name in program_names() {
            let source = stage.directory().join(name);
            copy_new(
                &source,
                &result.session.join("next").join(name),
                &fingerprint(&source, MAX_PROGRAM)?,
            )?;
        }
        result.verify_backups()?;
        result.verify_next(&result.session.join("next"))?;
        result.verify_old(system)?;
        result.save()?;
        Ok(result)
    }

    pub fn reopen(
        root: &Path,
        stage: &PortableStage,
        id: &str,
        system: &impl SystemState,
    ) -> Result<Self, AppError> {
        validate_id(id)?;
        plain(root, true)?;
        let root = root.canonicalize().map_err(|_| pending())?;
        system.validate_root(&root)?;
        if exists(&root.join("portable.txt"))? {
            return Err(pending());
        }
        stage.verify()?;
        let session = root.join(DIRECTORY);
        let receipt = receipt_path(&root, id);
        let completed = if exists(&receipt)? {
            let journal = load_journal(&receipt, system)?;
            if !matches!(journal.phase, Phase::Committed | Phase::RolledBack) {
                return Err(pending());
            }
            Some(journal)
        } else {
            None
        };
        let journal = if exists(&session)? {
            system.validate_session(&session)?;
            if exists(&session.join("journal.json"))? {
                let journal = load_journal(&session.join("journal.json"), system)?;
                if completed.as_ref().is_some_and(|value| *value != journal) {
                    return Err(pending());
                }
                journal
            } else {
                // Once the journal is removed, every other session file has
                // already been removed. Never adopt a new incomplete session.
                if fs::read_dir(&session)
                    .map_err(|_| pending())?
                    .next()
                    .is_some()
                {
                    return Err(pending());
                }
                completed.ok_or_else(pending)?
            }
        } else {
            completed.ok_or_else(pending)?
        };
        if journal.schema_version != 1
            || journal.id != id
            || journal.root != root
            || journal.next != *stage.index()
            || journal.previous_version >= journal.next.version
            || !journal.previous_version.build.is_empty()
            || journal
                .old_program
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != backup_names().collect()
            || ["Rela.exe", "uninstall.exe"]
                .iter()
                .any(|name| journal.old_program.get(*name).is_none_or(Option::is_none))
        {
            return Err(pending());
        }
        journal
            .before
            .validate_installation(&root, &journal.previous_version)?;
        for value in journal.old_program.values().flatten() {
            validate_fingerprint(value, MAX_PROGRAM)?;
        }
        if let Some(after) = &journal.after {
            after.validate_installation(&root, &journal.next.version)?;
        }
        if let Some(value) = &journal.new_uninstaller {
            validate_fingerprint(value, MAX_PROGRAM)?;
        }
        if journal.after.is_some() != journal.new_uninstaller.is_some()
            || (journal.phase == Phase::Prepared && journal.after.is_some())
            || (matches!(
                journal.phase,
                Phase::Installed | Phase::Committing | Phase::Committed
            ) && journal.after.is_none())
        {
            return Err(pending());
        }
        let result = Self {
            root,
            session,
            journal,
        };
        if !matches!(result.phase(), Phase::Committed | Phase::RolledBack) {
            result.verify_backups()?;
            result.verify_next(&result.session.join("next"))?;
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
        let bytes = serde_json::to_vec(&self.journal).map_err(|_| pending())?;
        if bytes.len() as u64 > JOURNAL_LIMIT {
            return Err(pending());
        }
        // A fixed internal slot also permits recovery from a partially written
        // journal without deleting any unrecognized file in the session.
        let next = self.session.join("journal.pending");
        remove_file(&next)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&next)
            .map_err(|_| pending())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| pending())?;
        drop(file);
        platform::replace_file(&next, &self.session.join("journal.json"))
    }

    /// The official NSIS post-install hook calls this only after all file and
    /// registry writes. Its absence never counts as installation success.
    pub fn installed(&mut self, system: &impl SystemState) -> Result<(), AppError> {
        if self.phase() == Phase::Installed {
            return self.verify_installed(system);
        }
        if self.phase() != Phase::Prepared {
            return Err(pending());
        }
        self.verify_backups()?;
        self.verify_next(&self.root)?;
        for name in backup_names() {
            system.validate_live_file(&self.root.join(name))?;
        }
        let after = system.snapshot()?;
        after.validate_installation(&self.root, &self.journal.next.version)?;
        let new_uninstaller = fingerprint(&self.root.join("uninstall.exe"), MAX_PROGRAM)?;
        if new_uninstaller.size < 2
            || !read_prefix(&self.root.join("uninstall.exe"))?.starts_with(b"MZ")
        {
            return Err(pending());
        }
        self.journal.after = Some(after);
        self.journal.new_uninstaller = Some(new_uninstaller);
        self.journal.phase = Phase::Installed;
        if let Err(error) = self.save() {
            self.journal.phase = Phase::Prepared;
            self.journal.after = None;
            self.journal.new_uninstaller = None;
            return Err(error);
        }
        Ok(())
    }

    /// Called after the hidden candidate and initiating user's configuration
    /// are ready, BEFORE any Core backup is discarded. It cannot be reversed.
    pub fn begin_commit(&mut self, system: &impl SystemState) -> Result<(), AppError> {
        if self.phase() == Phase::Committing {
            return self.verify_installed(system);
        }
        if self.phase() != Phase::Installed {
            return Err(pending());
        }
        self.verify_installed(system)?;
        self.set_phase(Phase::Committing)
    }

    /// Only after Core's matching durable commit receipt has been acknowledged.
    pub fn finish_commit(&mut self, system: &impl SystemState) -> Result<(), AppError> {
        if self.phase() == Phase::Committed {
            return self.verify_installed(system);
        }
        if self.phase() != Phase::Committing {
            return Err(pending());
        }
        self.verify_installed(system)?;
        self.set_phase(Phase::Committed)
    }

    fn set_phase(&mut self, next: Phase) -> Result<(), AppError> {
        let previous = self.journal.phase;
        self.journal.phase = next;
        if let Err(error) = self.save() {
            self.journal.phase = previous;
            return Err(error);
        }
        Ok(())
    }

    /// The controller first stops its candidate and verifies the installer has
    /// stopped writing. Core/configuration rollback is then independently retried.
    pub fn rollback(&mut self, system: &mut impl SystemState) -> Result<(), AppError> {
        self.rollback_observed(system, |_| Ok(()))
    }
    fn rollback_observed(
        &mut self,
        system: &mut impl SystemState,
        mut observe: impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        if self.phase() == Phase::RolledBack {
            return self.verify_old(system);
        }
        if !matches!(self.phase(), Phase::Prepared | Phase::Installed) {
            return Err(pending());
        }
        self.verify_backups()?;
        for (index, (name, old)) in self.journal.old_program.iter().enumerate() {
            let target = self.root.join(name);
            if let Some(old) = old {
                let parent = target.parent().ok_or_else(pending)?;
                if !exists(parent)? {
                    fs::create_dir(parent).map_err(|_| pending())?;
                }
                plain(parent, true)?;
                if exists(&target)? {
                    system.validate_live_file(&target)?;
                }
                let temporary = self.session.join("writing.tmp");
                remove_file(&temporary)?;
                copy_new(&self.session.join("old").join(name), &temporary, old)?;
                platform::replace_file(&temporary, &target)?;
                system.publish_restored_file(&target)?;
            } else if exists(&target)? {
                // Only fixed NSIS payload slots are ever removed. Arbitrary
                // files, data directories and unrelated installations remain.
                system.validate_live_file(&target)?;
                remove_file(&target)?;
            }
            observe(Step::RestoredFile(index))?;
        }
        for field in Field::ALL {
            system.restore_registry(field, self.journal.before.registry[&field].as_ref())?;
            observe(Step::RestoredRegistry(field))?;
        }
        for shortcut in Shortcut::ALL {
            system.restore_shortcut(
                shortcut,
                self.journal.before.shortcuts[&shortcut].as_deref(),
            )?;
            observe(Step::RestoredShortcut(shortcut))?;
        }
        self.verify_old(system)?;
        self.set_phase(Phase::RolledBack)?;
        observe(Step::RolledBack)
    }

    fn verify_backups(&self) -> Result<(), AppError> {
        for (name, value) in &self.journal.old_program {
            if fingerprint_optional(&self.session.join("old").join(name), MAX_PROGRAM)? != *value {
                return Err(pending());
            }
        }
        Ok(())
    }
    fn verify_next(&self, root: &Path) -> Result<(), AppError> {
        for name in program_names() {
            if fingerprint(&root.join(name), MAX_PROGRAM)?.sha256 != self.journal.next.files[name] {
                return Err(pending());
            }
        }
        Ok(())
    }
    fn verify_old(&self, system: &impl SystemState) -> Result<(), AppError> {
        for (name, value) in &self.journal.old_program {
            if fingerprint_optional(&self.root.join(name), MAX_PROGRAM)? != *value {
                return Err(pending());
            }
        }
        if system.snapshot()? != self.journal.before {
            return Err(pending());
        }
        Ok(())
    }
    fn verify_installed(&self, system: &impl SystemState) -> Result<(), AppError> {
        self.verify_next(&self.root)?;
        if fingerprint_optional(&self.root.join("uninstall.exe"), MAX_PROGRAM)?
            != self.journal.new_uninstaller
            || Some(system.snapshot()?) != self.journal.after
        {
            return Err(pending());
        }
        Ok(())
    }

    /// Called while the candidate remains gated. Partial cleanup is retryable;
    /// unknown contents or a locked snapshot preserve the terminal journal.
    pub fn cleanup(&self, system: &impl SystemState) -> Result<(), AppError> {
        match self.phase() {
            Phase::Committed => self.verify_installed(system)?,
            Phase::RolledBack => self.verify_old(system)?,
            _ => return Err(pending()),
        }
        let receipt = receipt_path(&self.root, &self.journal.id);
        if exists(&receipt)? && load_journal(&receipt, system)? != self.journal {
            return Err(pending());
        }
        if !exists(&self.session)? {
            return if exists(&receipt)? {
                Ok(())
            } else {
                Err(pending())
            };
        }
        system.validate_session(&self.session)?;
        let journal = self.session.join("journal.json");
        if !exists(&journal)? {
            // After loss of the final acknowledgement, only an empty session
            // can be removed using its protected terminal receipt.
            if !exists(&receipt)? {
                return Err(pending());
            }
            return fs::remove_dir(&self.session).map_err(|_| pending());
        }
        if load_journal(&journal, system)? != self.journal {
            return Err(pending());
        }
        for entry in fs::read_dir(&self.session).map_err(|_| pending())? {
            let entry = entry.map_err(|_| pending())?;
            let name = entry.file_name().into_string().map_err(|_| pending())?;
            if !matches!(
                name.as_str(),
                "old"
                    | "next"
                    | "journal.json"
                    | "journal.pending"
                    | "writing.tmp"
                    | "receipt.pending"
            ) {
                return Err(pending());
            }
            plain(&entry.path(), matches!(name.as_str(), "old" | "next"))?;
        }
        if !exists(&receipt)? {
            // This receipt outlives the fixed session path. Write it before
            // discarding any backup, so cleanup/acknowledgement loss is retryable.
            let temporary = self.session.join("receipt.pending");
            remove_file(&temporary)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| pending())?;
            let bytes = serde_json::to_vec(&self.journal).map_err(|_| pending())?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| pending())?;
            drop(file);
            system.validate_live_file(&temporary)?;
            platform::move_file_new(&temporary, &receipt)?;
            system.validate_live_file(&receipt)?;
        }
        let old = self
            .journal
            .old_program
            .iter()
            .filter_map(|(name, value)| {
                value
                    .as_ref()
                    .map(|value| (name.clone(), value.sha256.clone()))
            })
            .collect();
        remove_program_snapshot(&self.session.join("old"), &old)?;
        let next = program_names()
            .map(|name| (name.to_owned(), self.journal.next.files[name].clone()))
            .collect();
        remove_program_snapshot(&self.session.join("next"), &next)?;
        for name in ["writing.tmp", "journal.pending", "receipt.pending"] {
            remove_file(&self.session.join(name))?;
        }
        remove_file(&self.session.join("journal.json"))?;
        fs::remove_dir(&self.session).map_err(|_| pending())
    }
}

pub(super) fn receipt_path(root: &Path, id: &str) -> PathBuf {
    root.join(format!(".rela-installed-result-{id}.json"))
}

fn load_journal(path: &Path, system: &impl SystemState) -> Result<Journal, AppError> {
    system.validate_live_file(path)?;
    serde_json::from_slice(&read(path, JOURNAL_LIMIT)?).map_err(|_| pending())
}

fn probe_replace(path: &Path) -> Result<File, AppError> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .access_mode(0xC0010000)
        .share_mode(7)
        .open(path)
        .map_err(|_| {
            AppError::new(
                "update_files_in_use",
                "安装目录中的文件仍被占用或无法替换，请退出该目录的 Rela 后重试。",
            )
        })
}
fn read_prefix(path: &Path) -> Result<Vec<u8>, AppError> {
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| pending())?
        .take(2)
        .read_to_end(&mut bytes)
        .map_err(|_| pending())?;
    Ok(bytes)
}
pub(super) fn pending() -> AppError {
    AppError::new(
        "installer_recovery_required",
        "安装版更新或恢复尚未完成。备份已保留，请重试恢复，勿删除更新目录。",
    )
}

#[cfg(test)]
pub(in crate::updates) mod tests;
