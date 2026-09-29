//! Recoverable Portable file replacement. The caller must hold the update lock,
//! wait for the exact parent to exit, and reopen a signed package before using a
//! persisted transaction. No journal field grants permission to execute a file.
use crate::{
    distribution::portable::{PortableIndex, PortableStage, UPDATE_FILES},
    platform,
};
use rela_protocol::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const SESSION: &str = ".rela-update";
pub(super) const MAX_PROGRAM: u64 = 512 * 1024 * 1024;
const MAX_CONFIG_FILE: u64 = 64 * 1024 * 1024;
const MAX_CONFIG_TOTAL: u64 = 256 * 1024 * 1024;

/// Implemented by the process controller and the deferred, elevated Core helper.
/// All operations must tolerate retry after a process/power interruption.
pub trait Runtime {
    /// Stage/switch Core but retain its rollback state until commit is requested.
    fn prepare(&mut self) -> Result<(), AppError>;
    /// Stop only this transaction's child and wait for every config/file writer.
    fn quiesce(&mut self) -> Result<(), AppError>;
    /// Launch the verified candidate and require its bounded startup handshake.
    fn start_and_verify(&mut self, executable: &Path) -> Result<(), AppError>;
    fn rollback(&mut self) -> Result<(), AppError>;
    /// A durable commit decision has been written; errors here must roll forward.
    fn commit(&mut self) -> Result<(), AppError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Prepared,
    CorePrepared,
    Replaced(usize),
    Started,
    Committing,
    Committed,
    Restored(usize),
    ConfigDisplaced,
    ConfigRestored,
    RolledBack,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    Committing,
    Committed,
    RolledBack,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Fingerprint {
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Inventory {
    pub directories: BTreeSet<String>,
    pub files: BTreeMap<String, Fingerprint>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    id: String,
    phase: Phase,
    previous_version: Version,
    next: PortableIndex,
    old_program: BTreeMap<String, Option<Fingerprint>>,
    config_present: bool,
    config: Inventory,
}

pub struct Transaction {
    root: PathBuf,
    session: PathBuf,
    journal: Journal,
}

impl Transaction {
    /// Called only after all old GUI/config writers have exited. Preparation is
    /// non-destructive; an incomplete preparation contains no valid journal.
    pub fn prepare(
        root: &Path,
        stage: &PortableStage,
        previous_version: Version,
        id: &str,
    ) -> Result<Self, AppError> {
        validate_id(id)?;
        plain(root, true)?;
        plain(&root.join("portable.txt"), false)?;
        stage.verify()?;
        if previous_version >= stage.index().version || !previous_version.build.is_empty() {
            return Err(invalid());
        }
        let root = root.canonicalize().map_err(|_| invalid())?;
        let session = root.join(SESSION);
        if exists(&session)? {
            return Err(pending());
        }
        let mut old_program = BTreeMap::new();
        let mut needed = 32 * 1024 * 1024;
        let mut largest = 0;
        for name in program_names() {
            let old = fingerprint_optional(&root.join(&name), MAX_PROGRAM)?;
            if name == "Rela.exe" && old.is_none() {
                return Err(invalid());
            }
            if let Some(value) = &old {
                needed += value.size;
                largest = largest.max(value.size);
            }
            let size = fingerprint(&stage.directory().join(&name), MAX_PROGRAM)?.size;
            needed += size;
            largest = largest.max(size);
            old_program.insert(name, old);
        }
        let config_path = root.join("data/config");
        let config_present = exists(&config_path)?;
        let config = if config_present {
            inventory(&config_path)?
        } else {
            Inventory::default()
        };
        // Backup and rollback staging must both fit without consuming the only snapshot.
        needed += 2 * config.files.values().map(|f| f.size).sum::<u64>();
        needed += largest;
        if fs2::available_space(&root).map_err(|_| invalid())? < needed {
            return Err(AppError::new(
                "update_space_insufficient",
                "更新备份空间不足，原程序未更改。",
            ));
        }
        fs::create_dir(&session).map_err(|_| pending())?;
        let transaction = Self {
            root,
            session,
            journal: Journal {
                schema_version: 1,
                id: id.into(),
                phase: Phase::Prepared,
                previous_version,
                next: stage.index().clone(),
                old_program,
                config_present,
                config,
            },
        };
        for directory in ["old", "next"] {
            fs::create_dir(transaction.session.join(directory)).map_err(|_| pending())?;
            for child in ["easytier", "third-party-licenses"] {
                fs::create_dir(transaction.session.join(directory).join(child))
                    .map_err(|_| pending())?;
            }
        }
        for name in program_names() {
            if let Some(old) = &transaction.journal.old_program[&name] {
                copy_new(
                    &transaction.root.join(&name),
                    &transaction.session.join("old").join(&name),
                    old,
                )?;
            }
            let source = stage.directory().join(&name);
            copy_new(
                &source,
                &transaction.session.join("next").join(&name),
                &fingerprint(&source, MAX_PROGRAM)?,
            )?;
        }
        if config_present {
            copy_inventory(
                &config_path,
                &transaction.session.join("config-before"),
                &transaction.journal.config,
            )?;
        }
        transaction.verify_backups()?;
        transaction.verify_next()?;
        transaction.save()?;
        Ok(transaction)
    }

    /// The stage must come from a freshly verified signed package, not the journal.
    pub fn reopen(root: &Path, stage: &PortableStage, id: &str) -> Result<Self, AppError> {
        validate_id(id)?;
        plain(root, true)?;
        plain(&root.join("portable.txt"), false)?;
        stage.verify()?;
        let root = root.canonicalize().map_err(|_| pending())?;
        let session = root.join(SESSION);
        let journal: Journal =
            serde_json::from_slice(&read(&session.join("journal.json"), 2 * 1024 * 1024)?)
                .map_err(|_| pending())?;
        if journal.schema_version != 1
            || journal.id != id
            || journal.next != *stage.index()
            || journal.previous_version >= journal.next.version
            || journal.old_program.keys().cloned().collect::<BTreeSet<_>>()
                != program_names().collect()
            || journal
                .old_program
                .get("Rela.exe")
                .and_then(Option::as_ref)
                .is_none()
            || (!journal.config_present && journal.config != Inventory::default())
        {
            return Err(pending());
        }
        validate_inventory(&journal.config)?;
        for value in journal.old_program.values().flatten() {
            validate_fingerprint(value, MAX_PROGRAM)?;
        }
        let result = Self {
            root,
            session,
            journal,
        };
        if matches!(result.phase(), Phase::Prepared | Phase::Committing) {
            result.verify_backups()?;
            result.verify_next()?;
        }
        Ok(result)
    }

    pub fn phase(&self) -> Phase {
        self.journal.phase
    }
    pub fn recovery_directory(&self) -> &Path {
        &self.session
    }
    fn save(&self) -> Result<(), AppError> {
        plain(&self.session, true)?;
        platform::atomic_write(
            &self.session.join("journal.json"),
            &serde_json::to_vec(&self.journal).map_err(|_| pending())?,
        )
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
    pub fn execute(&mut self, runtime: &mut impl Runtime) -> Result<(), AppError> {
        self.execute_observed(runtime, |_| Ok(()))
    }
    fn execute_observed(
        &mut self,
        runtime: &mut impl Runtime,
        mut observe: impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        if self.phase() != Phase::Prepared {
            return Err(pending());
        }
        self.verify_backups()?;
        self.verify_next()?;
        self.verify_live_old()?;
        self.preflight()?;
        let result: Result<(), AppError> = (|| {
            observe(Step::Prepared)?;
            runtime.prepare()?;
            observe(Step::CorePrepared)?;
            for (index, name) in program_names().enumerate() {
                let source = self.session.join("next").join(&name);
                self.replace(
                    &source,
                    &self.root.join(&name),
                    &fingerprint(&source, MAX_PROGRAM)?,
                )?;
                observe(Step::Replaced(index))?;
            }
            self.verify_live_next()?;
            runtime.start_and_verify(&self.root.join("Rela.exe"))?;
            observe(Step::Started)?;
            // This is the durable point of no return, BEFORE deleting Core backups.
            self.set_phase(Phase::Committing)?;
            observe(Step::Committing)?;
            runtime.commit()?;
            self.set_phase(Phase::Committed)?;
            observe(Step::Committed)?;
            Ok(())
        })();
        if let Err(failure) = result {
            if self.phase() != Phase::Prepared {
                return Err(pending());
            }
            self.recover(runtime)?;
            return Err(AppError::new(
                "portable_update_rolled_back",
                &format!("{} 已恢复原程序和配置。", failure.message),
            ));
        }
        Ok(())
    }

    pub fn recover(&mut self, runtime: &mut impl Runtime) -> Result<(), AppError> {
        self.recover_observed(runtime, |_| Ok(()))
    }
    fn recover_observed(
        &mut self,
        runtime: &mut impl Runtime,
        mut observe: impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        if matches!(self.phase(), Phase::Prepared | Phase::Committing) {
            self.verify_backups()?;
        }
        match self.phase() {
            Phase::Committing => {
                self.verify_live_next()?;
                runtime.commit()?;
                self.set_phase(Phase::Committed)
            }
            Phase::Committed => self.verify_live_next(),
            Phase::RolledBack => self.verify_live_old(),
            Phase::Prepared => {
                runtime.quiesce()?;
                for (index, name) in program_names().enumerate() {
                    let target = self.root.join(&name);
                    match &self.journal.old_program[&name] {
                        Some(old) => {
                            self.replace(&self.session.join("old").join(&name), &target, old)?
                        }
                        None => remove_file(&target)?,
                    }
                    observe(Step::Restored(index))?;
                }
                self.restore_config(&mut observe)?;
                self.verify_live_old()?;
                runtime.rollback()?;
                self.set_phase(Phase::RolledBack)?;
                observe(Step::RolledBack)
            }
        }
    }
    fn replace(
        &self,
        source: &Path,
        target: &Path,
        expected: &Fingerprint,
    ) -> Result<(), AppError> {
        let parent = target.parent().ok_or_else(pending)?;
        if !exists(parent)? {
            fs::create_dir(parent).map_err(|_| pending())?;
        }
        plain(parent, true)?;
        if exists(target)? {
            plain(target, false)?;
        }
        let temporary = self.session.join("writing.tmp");
        remove_file(&temporary)?;
        copy_new(source, &temporary, expected)?;
        platform::replace_file(&temporary, target).map_err(|_| pending())
    }
    fn preflight(&self) -> Result<(), AppError> {
        for name in program_names() {
            let target = self.root.join(name);
            if exists(&target)? {
                plain(&target, false)?;
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(windows)]
                {
                    use std::os::windows::fs::OpenOptionsExt;
                    // Detect existing handles which deny replacement before changing any file.
                    options.access_mode(0x8001_0000).share_mode(7);
                }
                let _probe = options.open(target).map_err(|_| {
                    AppError::new(
                        "update_files_in_use",
                        "程序文件被占用或无替换权限，请退出其他 Rela 副本后重试。",
                    )
                })?;
            }
        }
        Ok(())
    }
    fn verify_backups(&self) -> Result<(), AppError> {
        for (name, expected) in &self.journal.old_program {
            if fingerprint_optional(&self.session.join("old").join(name), MAX_PROGRAM)? != *expected
            {
                return Err(pending());
            }
        }
        if self.journal.config_present
            && inventory(&self.session.join("config-before"))? != self.journal.config
        {
            return Err(pending());
        }
        Ok(())
    }
    fn verify_next(&self) -> Result<(), AppError> {
        crate::distribution::portable::verify_tree(&self.session.join("next"), &self.journal.next)
    }
    fn verify_live_next(&self) -> Result<(), AppError> {
        crate::distribution::portable::verify_program_files(&self.root, &self.journal.next)
    }
    fn verify_live_old(&self) -> Result<(), AppError> {
        for (name, expected) in &self.journal.old_program {
            if fingerprint_optional(&self.root.join(name), MAX_PROGRAM)? != *expected {
                return Err(pending());
            }
        }
        let config = self.root.join("data/config");
        if exists(&config)? != self.journal.config_present
            || (self.journal.config_present && inventory(&config)? != self.journal.config)
        {
            return Err(pending());
        }
        Ok(())
    }
    fn restore_config(
        &self,
        observe: &mut impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let current = self.root.join("data/config");
        let failed = self.session.join("config-failed");
        if exists(&current)? {
            if exists(&failed)? {
                // A prior recovery already restored the snapshot. Never discard later edits.
                if !self.journal.config_present || inventory(&current)? != self.journal.config {
                    return Err(pending());
                }
                return Ok(());
            }
            plain(&current, true)?;
            platform::replace_file(&current, &failed).map_err(|_| pending())?;
        }
        observe(Step::ConfigDisplaced)?;
        if self.journal.config_present {
            let restored = self.session.join("config-restore");
            copy_inventory(
                &self.session.join("config-before"),
                &restored,
                &self.journal.config,
            )?;
            let data = self.root.join("data");
            if !exists(&data)? {
                fs::create_dir(&data).map_err(|_| pending())?;
            }
            plain(&data, true)?;
            platform::replace_file(&restored, &current).map_err(|_| pending())?;
        }
        observe(Step::ConfigRestored)
    }

    /// Run while the candidate (or restored old app) is still behind the startup
    /// gate. Unknown files stop cleanup. Failed migration output is archived in data.
    /// Journal removal is last, so a crash during cleanup can safely retry.
    pub fn cleanup(&self) -> Result<(), AppError> {
        match self.phase() {
            Phase::Committed => self.verify_live_next()?,
            Phase::RolledBack => self.verify_live_old()?,
            _ => return Err(pending()),
        }
        if !exists(&self.session)? {
            return Ok(());
        }
        for entry in fs::read_dir(&self.session).map_err(|_| pending())? {
            let entry = entry.map_err(|_| pending())?;
            let name = entry.file_name().into_string().map_err(|_| pending())?;
            if !(matches!(
                name.as_str(),
                "old"
                    | "next"
                    | "config-before"
                    | "config-failed"
                    | "config-restore"
                    | "journal.json"
                    | "writing.tmp"
            ) || (name.starts_with(".rela-") && name.ends_with(".tmp")))
            {
                return Err(pending());
            }
            if !matches!(
                name.as_str(),
                "old" | "next" | "config-before" | "config-failed" | "config-restore"
            ) {
                plain(&entry.path(), false)?;
            }
        }
        let failed = self.session.join("config-failed");
        if exists(&failed)? {
            let content = inventory(&failed)?;
            if self.journal.config_present && content == self.journal.config {
                remove_inventory_snapshot(&failed, &self.journal.config)?;
            } else {
                let data = self.root.join("data");
                if !exists(&data)? {
                    fs::create_dir(&data).map_err(|_| pending())?;
                }
                plain(&data, true)?;
                let recovery = data.join("recovery");
                if !exists(&recovery)? {
                    fs::create_dir(&recovery).map_err(|_| pending())?;
                }
                plain(&recovery, true)?;
                let target = recovery.join(format!("update-{}", self.journal.id));
                if exists(&target)? {
                    return Err(pending());
                }
                platform::replace_file(&failed, &target).map_err(|_| pending())?;
            }
        }
        remove_program_snapshot(
            &self.session.join("old"),
            &self
                .journal
                .old_program
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .as_ref()
                        .map(|value| (name.clone(), value.sha256.clone()))
                })
                .collect(),
        )?;
        let mut next = self.journal.next.files.clone();
        let checksums = self.session.join("next/checksums.json");
        if exists(&checksums)? {
            let bytes = read(&checksums, 32768)?;
            if serde_json::from_slice::<PortableIndex>(&bytes).map_err(|_| pending())?
                != self.journal.next
            {
                return Err(pending());
            }
            next.insert(
                "checksums.json".into(),
                format!("{:x}", Sha256::digest(bytes)),
            );
        }
        remove_program_snapshot(&self.session.join("next"), &next)?;
        for name in ["config-before", "config-restore"] {
            remove_inventory_snapshot(&self.session.join(name), &self.journal.config)?;
        }
        for entry in fs::read_dir(&self.session).map_err(|_| pending())? {
            let entry = entry.map_err(|_| pending())?;
            if entry.file_name() != "journal.json" {
                remove_file(&entry.path())?;
            }
        }
        remove_file(&self.session.join("journal.json"))?;
        fs::remove_dir(&self.session).map_err(|_| pending())
    }
}

pub(super) fn remove_program_snapshot(
    root: &Path,
    expected: &BTreeMap<String, String>,
) -> Result<(), AppError> {
    if !exists(root)? {
        return Ok(());
    }
    plain(root, true)?;
    let mut files = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| pending())? {
        let entry = entry.map_err(|_| pending())?;
        let name = entry.file_name().into_string().map_err(|_| pending())?;
        if matches!(name.as_str(), "easytier" | "third-party-licenses") {
            plain(&entry.path(), true)?;
            for child in fs::read_dir(entry.path()).map_err(|_| pending())? {
                let child = child.map_err(|_| pending())?;
                files.push(format!(
                    "{name}/{}",
                    child.file_name().into_string().map_err(|_| pending())?
                ));
            }
        } else {
            files.push(name);
        }
    }
    for name in &files {
        let Some(hash) = expected.get(name) else {
            return Err(pending());
        };
        if fingerprint(&root.join(name), MAX_PROGRAM)?.sha256 != *hash {
            return Err(pending());
        }
    }
    for name in files {
        remove_file(&root.join(name))?;
    }
    for directory in ["easytier", "third-party-licenses"] {
        if exists(&root.join(directory))? {
            fs::remove_dir(root.join(directory)).map_err(|_| pending())?;
        }
    }
    fs::remove_dir(root).map_err(|_| pending())
}
pub(super) fn remove_inventory_snapshot(root: &Path, expected: &Inventory) -> Result<(), AppError> {
    if !exists(root)? {
        return Ok(());
    }
    let remaining = inventory(root)?;
    if !remaining.directories.is_subset(&expected.directories)
        || remaining
            .files
            .iter()
            .any(|(name, value)| expected.files.get(name) != Some(value))
    {
        return Err(pending());
    }
    for name in remaining.files.keys() {
        remove_file(&root.join(name))?;
    }
    for name in remaining.directories.iter().rev() {
        fs::remove_dir(root.join(name)).map_err(|_| pending())?;
    }
    fs::remove_dir(root).map_err(|_| pending())
}

pub(super) fn validate_id(id: &str) -> Result<(), AppError> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid());
    }
    Ok(())
}

fn program_names() -> impl Iterator<Item = String> {
    UPDATE_FILES
        .iter()
        .map(|name| name.to_string())
        .chain(std::iter::once("checksums.json".into()))
}

pub(super) fn fingerprint_optional(
    path: &Path,
    limit: u64,
) -> Result<Option<Fingerprint>, AppError> {
    if exists(path)? {
        Ok(Some(fingerprint(path, limit)?))
    } else {
        Ok(None)
    }
}
pub(super) fn fingerprint(path: &Path, limit: u64) -> Result<Fingerprint, AppError> {
    plain(path, false)?;
    let mut reader = File::open(path).map_err(|_| pending())?.take(limit + 1);
    let mut hash = Sha256::new();
    let size = std::io::copy(&mut reader, &mut hash).map_err(|_| pending())?;
    if size > limit {
        return Err(pending());
    }
    Ok(Fingerprint {
        size,
        sha256: format!("{:x}", hash.finalize()),
    })
}
pub(super) fn copy_new(
    source: &Path,
    target: &Path,
    expected: &Fingerprint,
) -> Result<(), AppError> {
    plain(source, false)?;
    plain(target.parent().ok_or_else(pending)?, true)?;
    let mut source = File::open(source)
        .map_err(|_| pending())?
        .take(expected.size + 1);
    let mut target = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(|_| pending())?;
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut bytes = [0u8; 65536];
    loop {
        let count = source.read(&mut bytes).map_err(|_| pending())?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > expected.size {
            return Err(pending());
        }
        hash.update(&bytes[..count]);
        target.write_all(&bytes[..count]).map_err(|_| pending())?;
    }
    if total != expected.size || format!("{:x}", hash.finalize()) != expected.sha256 {
        return Err(pending());
    }
    target.sync_all().map_err(|_| pending())
}

pub(super) fn inventory(root: &Path) -> Result<Inventory, AppError> {
    plain(root, true)?;
    let mut result = Inventory::default();
    let mut pending_dirs = vec![(root.to_path_buf(), String::new(), 0)];
    let mut total = 0u64;
    while let Some((directory, relative, depth)) = pending_dirs.pop() {
        if depth > 8 {
            return Err(pending());
        }
        for entry in fs::read_dir(directory).map_err(|_| pending())? {
            let entry = entry.map_err(|_| pending())?;
            let name = entry.file_name().into_string().map_err(|_| pending())?;
            let name = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            validate_relative(&name)?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|_| pending())?;
            plain(&entry.path(), metadata.is_dir())?;
            if metadata.is_dir() {
                result.directories.insert(name.clone());
                pending_dirs.push((entry.path(), name, depth + 1));
            } else {
                let value = fingerprint(&entry.path(), MAX_CONFIG_FILE)?;
                total += value.size;
                result.files.insert(name, value);
            }
            if result.files.len() + result.directories.len() > 4096 || total > MAX_CONFIG_TOTAL {
                return Err(pending());
            }
        }
    }
    Ok(result)
}
fn validate_relative(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 2048
        || value.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || part.chars().any(|c| c < ' ' || "\\:<>\"|?*".contains(c))
        })
    {
        return Err(pending());
    }
    Ok(())
}
pub(super) fn validate_fingerprint(value: &Fingerprint, limit: u64) -> Result<(), AppError> {
    if value.size > limit
        || value.sha256.len() != 64
        || !value
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(pending());
    }
    Ok(())
}
pub(super) fn validate_inventory(value: &Inventory) -> Result<(), AppError> {
    if value.files.len() + value.directories.len() > 4096 {
        return Err(pending());
    }
    for name in value.files.keys().chain(value.directories.iter()) {
        validate_relative(name)?;
    }
    for file in value.files.values() {
        validate_fingerprint(file, MAX_CONFIG_FILE)?;
    }
    if value.files.values().map(|f| f.size).sum::<u64>() > MAX_CONFIG_TOTAL {
        return Err(pending());
    }
    Ok(())
}
pub(super) fn copy_inventory(
    source: &Path,
    target: &Path,
    expected: &Inventory,
) -> Result<(), AppError> {
    if inventory(source)? != *expected {
        return Err(pending());
    }
    if exists(target)? {
        let partial = inventory(target)?;
        if partial
            .files
            .keys()
            .any(|name| !expected.files.contains_key(name))
            || !partial.directories.is_subset(&expected.directories)
        {
            return Err(pending());
        }
    } else {
        fs::create_dir(target).map_err(|_| pending())?;
    }
    for name in &expected.directories {
        if !exists(&target.join(name))? {
            fs::create_dir(target.join(name)).map_err(|_| pending())?;
        }
    }
    for (name, file) in &expected.files {
        let path = target.join(name);
        if fingerprint_optional(&path, MAX_CONFIG_FILE)? != Some(file.clone()) {
            remove_file(&path)?;
            copy_new(&source.join(name), &path, file)?;
        }
    }
    if inventory(target)? != *expected {
        return Err(pending());
    }
    Ok(())
}

pub(super) fn exists(path: &Path) -> Result<bool, AppError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            plain(
                path,
                fs::symlink_metadata(path).map_err(|_| pending())?.is_dir(),
            )?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // A missing leaf does not make a redirected ancestor safe.
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                if exists(parent)? {
                    plain(parent, true)?;
                }
            }
            Ok(false)
        }
        Err(_) => Err(pending()),
    }
}
pub(super) fn plain(path: &Path, directory: bool) -> Result<(), AppError> {
    for (index, ancestor) in path
        .ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .enumerate()
    {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| pending())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(pending());
            }
        }
        if metadata.file_type().is_symlink()
            || (index == 0
                && (metadata.is_dir() != directory || (!directory && !metadata.is_file())))
            || (index > 0 && !metadata.is_dir())
        {
            return Err(pending());
        }
    }
    Ok(())
}
pub(super) fn read(path: &Path, limit: u64) -> Result<Vec<u8>, AppError> {
    plain(path, false)?;
    let mut value = Vec::new();
    File::open(path)
        .map_err(|_| pending())?
        .take(limit + 1)
        .read_to_end(&mut value)
        .map_err(|_| pending())?;
    if value.len() as u64 > limit {
        return Err(pending());
    }
    Ok(value)
}
pub(super) fn remove_file(path: &Path) -> Result<(), AppError> {
    if exists(path)? {
        plain(path, false)?;
        fs::remove_file(path).map_err(|_| pending())?;
    }
    Ok(())
}
fn invalid() -> AppError {
    AppError::new(
        "portable_update_invalid",
        "无法验证绿色版更新目标，原程序未更改。",
    )
}
fn pending() -> AppError {
    AppError::new(
        "portable_recovery_required",
        "更新恢复尚未完成。备份已保留，请重试或联系管理员，勿删除更新目录。",
    )
}

#[cfg(test)]
mod tests;
