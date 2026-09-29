//! Complete engine/config transactions under the protected service root.
//! Tests use real temporary files; SCM, ACL and TUN still require system acceptance.
use super::{integrity_error, AssetManifest, FILES};
use crate::platform;
use rela_protocol::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};

const RECORD: &str = "deployment.json";
const TRANSACTION: &str = "engine-transaction";
const APPLICATION_RECEIPT: &str = "application-update.json";
const DESCRIPTION_PREFIX: &str = "Rela network deployment v1: ";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Location {
    Legacy,
    Managed,
}
impl Location {
    pub fn directory(self, root: &Path) -> PathBuf {
        root.join(match self {
            Self::Legacy => "engine",
            Self::Managed => "engine-v2",
        })
    }
    pub fn executable(self, root: &Path) -> PathBuf {
        self.directory(root).join("easytier-core.exe")
    }
}

pub(super) fn allowed_binaries(root: &Path) -> Vec<PathBuf> {
    [Location::Managed, Location::Legacy]
        .iter()
        .map(|location| location.executable(root))
        .collect()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Deployment {
    pub schema_version: u32,
    pub owner_app_version: Version,
    pub assets: AssetManifest,
}
impl Deployment {
    pub fn bundled() -> Result<Self, AppError> {
        let assets = super::asset_manifest()?;
        Ok(Self {
            schema_version: 1,
            owner_app_version: Version::parse(env!("CARGO_PKG_VERSION"))
                .map_err(|_| integrity_error())?,
            assets,
        })
    }
    fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != 1 || !self.owner_app_version.build.is_empty() {
            return Err(integrity_error());
        }
        self.assets.validate()
    }
    pub fn public(&self) -> PublicDeployment {
        PublicDeployment {
            core_version: self.assets.version.clone(),
            owner_app_version: self.owner_app_version.clone(),
            engine_revision: self.assets.engine_revision,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PublicDeployment {
    pub core_version: String,
    pub owner_app_version: Version,
    pub engine_revision: u64,
}
impl PublicDeployment {
    pub fn from_description(description: &str) -> Option<Self> {
        let json = description.strip_prefix(DESCRIPTION_PREFIX)?;
        if json.len() > 1024 {
            return None;
        }
        let result: Self = serde_json::from_str(json).ok()?;
        if super::status::version(&result.core_version).is_none()
            || !result.owner_app_version.build.is_empty()
            || result.engine_revision == 0
            || result.engine_revision > rela_manifests::MAX_REVISION
        {
            return None;
        }
        Some(result)
    }
    pub fn description(&self) -> Result<String, AppError> {
        Ok(format!(
            "{DESCRIPTION_PREFIX}{}",
            serde_json::to_string(self).map_err(|_| integrity_error())?
        ))
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServiceSnapshot {
    pub location: Option<Location>,
    pub was_running: bool,
    pub description: Option<String>,
}

pub(super) trait Service {
    fn snapshot(&mut self) -> Result<ServiceSnapshot, AppError>;
    fn stop(&mut self) -> Result<(), AppError>;
    fn register(&mut self, location: Location) -> Result<(), AppError>;
    fn start_and_verify(&mut self, version: &str) -> Result<(), AppError>;
    fn verify_running(&mut self, version: &str) -> Result<(), AppError>;
    fn restore_registration(&mut self, previous: &ServiceSnapshot) -> Result<(), AppError>;
    fn publish(&mut self, deployment: &PublicDeployment) -> Result<(), AppError>;
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    AwaitingApplication,
    Committed,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Decision {
    Commit,
    Rollback,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplicationReceipt {
    schema_version: u32,
    id: String,
    decision: Decision,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: u32,
    phase: Phase,
    old_location: Option<Location>,
    old_deployment: Option<Deployment>,
    old_config_present: bool,
    old_config_credential: bool,
    previous_service: ServiceSnapshot,
    next: Deployment,
    #[serde(default)]
    application_id: Option<String>,
    #[serde(default)]
    next_config_sha256: Option<String>,
    #[serde(default)]
    old_config_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    Prepared,
    Stopped,
    EngineBackedUp,
    ConfigBackedUp,
    EngineReplaced,
    ConfigReplaced,
    Registered,
    Started,
    Published,
    AwaitingApplication,
    Committed,
}

pub(super) struct Files {
    root: PathBuf,
    protect_directory: fn(&Path) -> Result<(), AppError>,
    protect_file: fn(&Path) -> Result<(), AppError>,
}
impl Files {
    pub fn new(
        root: PathBuf,
        protect_directory: fn(&Path) -> Result<(), AppError>,
        protect_file: fn(&Path) -> Result<(), AppError>,
    ) -> Self {
        Self {
            root,
            protect_directory,
            protect_file,
        }
    }
    fn transaction(&self) -> PathBuf {
        self.root.join(TRANSACTION)
    }
    fn journal_path(&self) -> PathBuf {
        self.transaction().join("journal.json")
    }
    fn config(&self) -> PathBuf {
        self.root.join("core.toml")
    }
    fn secure_dir(&self, path: &Path) -> Result<(), AppError> {
        if path != self.root && !path.starts_with(&self.root) {
            return Err(recovery_error());
        }
        plain(path)?;
        (self.protect_directory)(path)?;
        if !fs::metadata(path).map_err(|_| recovery_error())?.is_dir() {
            return Err(recovery_error());
        }
        Ok(())
    }
    fn write_journal(&self, journal: &Journal) -> Result<(), AppError> {
        platform::atomic_write(
            &self.journal_path(),
            &serde_json::to_vec(journal).map_err(|_| recovery_error())?,
        )
    }

    pub fn recover(
        &self,
        service: &mut impl Service,
        restore_running: bool,
    ) -> Result<(), AppError> {
        let Some(journal) = self.read_journal()? else {
            return Ok(());
        };
        if journal.application_id.is_some() {
            return Err(application_pending());
        }
        if journal.phase == Phase::Committed {
            self.verify_live(&journal.next)?;
            service.publish(&journal.next.public())?;
            return self.cleanup();
        }
        self.rollback(&journal, service, restore_running)
    }

    fn read_journal(&self) -> Result<Option<Journal>, AppError> {
        self.secure_dir(&self.root)?;
        if plain(&self.config())? {
            (self.protect_file)(&self.config())?;
        }
        let transaction = self.transaction();
        if !plain(&transaction)? {
            return Ok(None);
        }
        self.secure_dir(&transaction)?;
        for entry in fs::read_dir(&transaction).map_err(|_| recovery_error())? {
            let entry = entry.map_err(|_| recovery_error())?;
            plain(&entry.path())?;
            if entry.file_type().map_err(|_| recovery_error())?.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some("new-engine" | "old-engine")
                ) {
                    return Err(recovery_error());
                }
                self.secure_dir(&entry.path())?;
                engine_contents(&entry.path())?;
                for file in fs::read_dir(entry.path()).map_err(|_| recovery_error())? {
                    (self.protect_file)(&file.map_err(|_| recovery_error())?.path())?;
                }
            } else {
                (self.protect_file)(&entry.path())?;
            }
        }
        if !plain(&self.journal_path())? {
            // No journal means preparation never touched live files. Backups contradict that.
            if plain(&transaction.join("old-engine"))? || plain(&transaction.join("old-core.toml"))?
            {
                return Err(recovery_error());
            }
            self.cleanup()?;
            return Ok(None);
        }
        let journal: Journal = serde_json::from_slice(&read(&self.journal_path(), 32 * 1024)?)
            .map_err(|_| recovery_error())?;
        if journal.schema_version != 1 {
            return Err(recovery_error());
        }
        journal.next.validate()?;
        if journal.next.owner_app_version
            > Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| integrity_error())?
        {
            return Err(AppError::new(
                "core_downgrade_blocked",
                "请使用发起引擎更新的较新 Rela 版本完成恢复。",
            ));
        }
        if let Some(old) = &journal.old_deployment {
            old.validate()?;
        }
        if journal.old_location.is_some() != journal.old_deployment.is_some()
            || journal
                .previous_service
                .description
                .as_ref()
                .is_some_and(|value| value.len() > 4096)
        {
            return Err(recovery_error());
        }
        if let Some(id) = &journal.application_id {
            validate_application_id(id)?;
            if journal.next_config_sha256.is_none() || journal.old_config_sha256.is_none() {
                return Err(recovery_error());
            }
        }
        for hash in [&journal.next_config_sha256, &journal.old_config_sha256]
            .into_iter()
            .flatten()
        {
            validate_application_id(hash)?;
        }
        if journal.phase == Phase::AwaitingApplication && journal.application_id.is_none() {
            return Err(recovery_error());
        }
        Ok(Some(journal))
    }

    fn write_receipt(&self, id: &str, decision: Decision) -> Result<(), AppError> {
        let path = self.root.join(APPLICATION_RECEIPT);
        if plain(&path)? {
            (self.protect_file)(&path)?;
        }
        platform::atomic_write(
            &path,
            &serde_json::to_vec(&ApplicationReceipt {
                schema_version: 1,
                id: id.into(),
                decision,
            })
            .map_err(|_| recovery_error())?,
        )
    }
    fn receipt(&self, id: &str) -> Result<Option<Decision>, AppError> {
        let path = self.root.join(APPLICATION_RECEIPT);
        if !plain(&path)? {
            return Ok(None);
        }
        (self.protect_file)(&path)?;
        let receipt: ApplicationReceipt =
            serde_json::from_slice(&read(&path, 1024)?).map_err(|_| recovery_error())?;
        if receipt.schema_version != 1 {
            return Err(recovery_error());
        }
        validate_application_id(&receipt.id)?;
        Ok((receipt.id == id).then_some(receipt.decision))
    }

    pub fn prepare_application(
        &self,
        bundled: &Path,
        id: &str,
        service: &mut impl Service,
    ) -> Result<bool, AppError> {
        self.prepare_application_observed(bundled, id, Deployment::bundled()?, service, |_| Ok(()))
    }
    fn prepare_application_observed(
        &self,
        bundled: &Path,
        id: &str,
        next: Deployment,
        service: &mut impl Service,
        observer: impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<bool, AppError> {
        validate_application_id(id)?;
        if let Some(journal) = self.read_journal()? {
            if journal.application_id.is_none() {
                self.recover(service, true)?;
            } else {
                if journal.application_id.as_deref() != Some(id)
                    || journal.phase != Phase::AwaitingApplication
                {
                    return Err(application_pending());
                }
                if serde_json::to_vec(&journal.next).ok() != serde_json::to_vec(&next).ok() {
                    return Err(integrity_error());
                }
                self.application_ready(&journal, service)?;
                return Ok(true);
            }
        }
        if self.receipt(id)?.is_some() {
            return Err(application_pending());
        }
        // Updating the GUI does not install a service or initiate a new connection.
        if service.snapshot()?.location.is_none() {
            return Ok(false);
        }
        let config = read(&self.config(), 64 * 1024)?;
        if !credential_config(&config) {
            return Err(AppError::new(
                "credential_migration_required",
                "请先填写 credential 并完成网络配置迁移，再更新网络引擎。",
            ));
        }
        self.apply_inner(bundled, &config, next, Some(id), service, observer)?;
        Ok(true)
    }

    /// The application controller supplies its durable decision. A lost helper
    /// connection leaves this journal pending; timeout alone never chooses rollback.
    pub fn resolve_application(
        &self,
        id: &str,
        decision: Decision,
        service: &mut impl Service,
    ) -> Result<(), AppError> {
        validate_application_id(id)?;
        let Some(mut journal) = self.read_journal()? else {
            let recorded = self.receipt(id)?;
            return if recorded == Some(decision)
                || (recorded.is_none() && decision == Decision::Rollback)
            {
                Ok(())
            } else {
                Err(application_pending())
            };
        };
        if journal.application_id.as_deref() != Some(id) {
            return Err(application_pending());
        }
        if let Some(recorded) = self.receipt(id)? {
            if recorded != decision {
                return Err(application_pending());
            }
        }
        match decision {
            Decision::Commit => {
                if journal.phase == Phase::Prepared {
                    return Err(application_pending());
                }
                self.application_ready(&journal, service)?;
                if journal.phase != Phase::Committed {
                    journal.phase = Phase::Committed;
                    self.write_journal(&journal)?;
                }
                self.write_receipt(id, Decision::Commit)?;
                self.cleanup()
            }
            Decision::Rollback => {
                if journal.phase == Phase::Committed {
                    return Err(application_pending());
                }
                self.rollback(&journal, service, true)
            }
        }
    }
    fn application_ready(
        &self,
        journal: &Journal,
        service: &mut impl Service,
    ) -> Result<(), AppError> {
        self.verify_live(&journal.next)?;
        if journal.next_config_sha256.as_ref() != Some(&digest(&read(&self.config(), 64 * 1024)?)) {
            return Err(integrity_error());
        }
        let actual = service.snapshot()?;
        if actual.location != Some(Location::Managed) {
            return Err(recovery_error());
        }
        if journal.previous_service.was_running {
            if !actual.was_running {
                service.start_and_verify(&journal.next.assets.version)?;
            } else {
                service.verify_running(&journal.next.assets.version)?;
            }
        } else if actual.was_running {
            return Err(recovery_error());
        }
        service.publish(&journal.next.public())
    }

    fn existing(
        &self,
        service: &ServiceSnapshot,
    ) -> Result<Option<(Location, Deployment)>, AppError> {
        let managed = plain(&Location::Managed.directory(&self.root))?;
        let legacy = plain(&Location::Legacy.directory(&self.root))?;
        if managed && legacy {
            return Err(recovery_error());
        }
        let location = if managed {
            Some(Location::Managed)
        } else if legacy {
            Some(Location::Legacy)
        } else {
            None
        };
        if service.location.is_some() && service.location != location {
            return Err(recovery_error());
        }
        let Some(location) = location else {
            return Ok(None);
        };
        let directory = location.directory(&self.root);
        self.secure_dir(&directory)?;
        engine_contents(&directory)?;
        for entry in fs::read_dir(&directory).map_err(|_| recovery_error())? {
            (self.protect_file)(&entry.map_err(|_| recovery_error())?.path())?;
        }
        let record = directory.join(RECORD);
        let deployment = if plain(&record)? {
            let deployment: Deployment = serde_json::from_slice(&read(&record, 16 * 1024)?)
                .map_err(|_| integrity_error())?;
            deployment.validate()?;
            deployment.assets.verify(&directory)?;
            deployment
        } else if location == Location::Legacy {
            // Only two fully pinned historical sets can enter record-based management.
            let known = [
                super::asset_manifest()?,
                serde_json::from_str::<AssetManifest>(include_str!(
                    "../../../config/easytier-legacy.json"
                ))
                .map_err(|_| integrity_error())?,
            ];
            let assets = known
                .into_iter()
                .find(|assets| assets.verify(&directory).is_ok())
                .ok_or_else(|| {
                    AppError::new(
                        "core_version_unknown",
                        "现有引擎版本无法验证，请使用较新的 Rela 或联系管理员恢复。",
                    )
                })?;
            Deployment {
                schema_version: 1,
                owner_app_version: Version::new(0, 0, 0),
                assets,
            }
        } else {
            return Err(integrity_error());
        };
        Ok(Some((location, deployment)))
    }

    pub fn apply(
        &self,
        bundled: &Path,
        config: &[u8],
        service: &mut impl Service,
    ) -> Result<(), AppError> {
        self.apply_observed(bundled, config, Deployment::bundled()?, service, |_| Ok(()))
    }

    fn apply_observed(
        &self,
        bundled: &Path,
        config: &[u8],
        next: Deployment,
        service: &mut impl Service,
        observer: impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        self.apply_inner(bundled, config, next, None, service, observer)
    }

    fn apply_inner(
        &self,
        bundled: &Path,
        config: &[u8],
        next: Deployment,
        application_id: Option<&str>,
        service: &mut impl Service,
        mut observer: impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        self.recover(service, true)?;
        next.validate()?;
        next.assets.verify(bundled)?;
        if config.len() > 64 * 1024 || !credential_config(config) {
            return Err(integrity_error());
        }
        let previous_service = service.snapshot()?;
        let previous = self.existing(&previous_service)?;
        if let Some((_, old)) = &previous {
            upgrade_allowed(old, &next)?;
        }
        let old_config_present = plain(&self.config())?;
        let old_config = if old_config_present {
            Some(read(&self.config(), 64 * 1024)?)
        } else {
            None
        };
        let old_config_credential = old_config
            .as_ref()
            .is_some_and(|bytes| credential_config(bytes));
        if previous_service.location.is_some() && (!old_config_present || previous.is_none()) {
            return Err(recovery_error());
        }
        let transaction = self.transaction();
        self.secure_dir(&transaction)?;
        let staged_engine = transaction.join("new-engine");
        self.secure_dir(&staged_engine)?;
        for name in FILES {
            // Copy into the protected directory, then verify again before execution.
            // Create with the protected parent's ACL rather than copying source security attributes.
            let source = fs::File::open(bundled.join(name)).map_err(|_| integrity_error())?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(staged_engine.join(name))
                .map_err(|_| platform::storage_error())?;
            let copied = std::io::copy(&mut source.take(256 * 1024 * 1024 + 1), &mut output)
                .map_err(|_| platform::storage_error())?;
            if copied > 256 * 1024 * 1024 {
                return Err(integrity_error());
            }
            output.sync_all().map_err(|_| platform::storage_error())?;
            drop(output);
            (self.protect_file)(&staged_engine.join(name))?;
        }
        next.assets.verify(&staged_engine)?;
        platform::atomic_write(
            &staged_engine.join(RECORD),
            &serde_json::to_vec(&next).map_err(|_| integrity_error())?,
        )?;
        platform::atomic_write(&transaction.join("new-core.toml"), config)?;
        let mut journal = Journal {
            schema_version: 1,
            phase: Phase::Prepared,
            old_location: previous.as_ref().map(|(location, _)| *location),
            old_deployment: previous.map(|(_, deployment)| deployment),
            old_config_present,
            old_config_credential,
            previous_service,
            next,
            application_id: application_id.map(str::to_owned),
            next_config_sha256: Some(digest(config)),
            old_config_sha256: old_config.as_ref().map(|bytes| digest(bytes)),
        };
        self.write_journal(&journal)?;
        let result = (|| {
            observer(Step::Prepared)?;
            service.stop()?;
            observer(Step::Stopped)?;
            if let Some(location) = journal.old_location {
                move_path(
                    &location.directory(&self.root),
                    &transaction.join("old-engine"),
                )?;
            }
            observer(Step::EngineBackedUp)?;
            if journal.old_config_present {
                move_path(&self.config(), &transaction.join("old-core.toml"))?;
            }
            observer(Step::ConfigBackedUp)?;
            move_path(&staged_engine, &Location::Managed.directory(&self.root))?;
            observer(Step::EngineReplaced)?;
            move_path(&transaction.join("new-core.toml"), &self.config())?;
            observer(Step::ConfigReplaced)?;
            self.verify_live(&journal.next)?;
            service.register(Location::Managed)?;
            observer(Step::Registered)?;
            if application_id.is_none() || journal.previous_service.was_running {
                service.start_and_verify(&journal.next.assets.version)?;
            }
            observer(Step::Started)?;
            service.publish(&journal.next.public())?;
            observer(Step::Published)?;
            journal.phase = if application_id.is_some() {
                Phase::AwaitingApplication
            } else {
                Phase::Committed
            };
            if let Err(error) = self.write_journal(&journal) {
                journal.phase = Phase::Prepared;
                return Err(error);
            }
            observer(if application_id.is_some() {
                Step::AwaitingApplication
            } else {
                Step::Committed
            })?;
            Ok(())
        })();
        if let Err(error) = result {
            if journal.phase == Phase::Committed {
                self.cleanup()?;
                return Err(error);
            }
            self.rollback(&journal, service, true)?;
            return Err(AppError::new(
                "core_update_rolled_back",
                if error.code == "service_conflict" {
                    "网络服务或管理端口冲突，已恢复更新前的引擎和配置。"
                } else {
                    "网络引擎更新未完成，已恢复更新前的引擎和配置。"
                },
            ));
        }
        // A committed journal is kept if cleanup fails; next operation safely retries cleanup.
        if application_id.is_some() {
            Ok(())
        } else {
            self.cleanup()
        }
    }

    fn verify_live(&self, deployment: &Deployment) -> Result<(), AppError> {
        deployment
            .assets
            .verify(&Location::Managed.directory(&self.root))?;
        let actual: Deployment = serde_json::from_slice(&read(
            &Location::Managed.directory(&self.root).join(RECORD),
            16 * 1024,
        )?)
        .map_err(|_| integrity_error())?;
        if serde_json::to_vec(&actual).ok() != serde_json::to_vec(deployment).ok() {
            return Err(integrity_error());
        }
        Ok(())
    }

    fn rollback(
        &self,
        journal: &Journal,
        service: &mut impl Service,
        restore_running: bool,
    ) -> Result<(), AppError> {
        service.stop().map_err(|_| recovery_error())?;
        let transaction = self.transaction();
        let backup_engine = transaction.join("old-engine");
        let live_engine = Location::Managed.directory(&self.root);
        if plain(&backup_engine)? {
            let old = journal.old_deployment.as_ref().ok_or_else(recovery_error)?;
            old.assets.verify(&backup_engine)?;
            remove_engine(&live_engine)?;
            let original = journal
                .old_location
                .ok_or_else(recovery_error)?
                .directory(&self.root);
            if plain(&original)? {
                return Err(recovery_error());
            }
            move_path(&backup_engine, &original)?;
        } else if journal.old_location.is_none() {
            remove_engine(&live_engine)?;
        } else {
            // Backup is absent either before the move or after a previous partial recovery.
            let old = journal.old_deployment.as_ref().ok_or_else(recovery_error)?;
            old.assets
                .verify(&journal.old_location.unwrap().directory(&self.root))?;
        }
        let backup_config = transaction.join("old-core.toml");
        if plain(&backup_config)? {
            if let Some(expected) = &journal.old_config_sha256 {
                if digest(&read(&backup_config, 64 * 1024)?) != *expected {
                    return Err(recovery_error());
                }
            }
            remove_file(&self.config())?;
            move_path(&backup_config, &self.config())?;
        } else if !journal.old_config_present {
            remove_file(&self.config())?;
        } else if !plain(&self.config())? {
            return Err(recovery_error());
        }
        if let Some(expected) = &journal.old_config_sha256 {
            if digest(&read(&self.config(), 64 * 1024)?) != *expected {
                return Err(recovery_error());
            }
        }
        service
            .restore_registration(&journal.previous_service)
            .map_err(|_| recovery_error())?;
        if restore_running && journal.previous_service.was_running && journal.old_config_credential
        {
            service
                .start_and_verify(
                    &journal
                        .old_deployment
                        .as_ref()
                        .ok_or_else(recovery_error)?
                        .assets
                        .version,
                )
                .map_err(|_| recovery_error())?;
        }
        if let Some(id) = &journal.application_id {
            self.write_receipt(id, Decision::Rollback)?;
        }
        self.cleanup()?;
        if restore_running && journal.previous_service.was_running && !journal.old_config_credential
        {
            return Err(AppError::new(
                "credential_migration_required",
                "已保留旧引擎和配置，但未恢复旧密码连接。请填写 credential 后重新连接。",
            ));
        }
        Ok(())
    }

    fn cleanup(&self) -> Result<(), AppError> {
        let transaction = self.transaction();
        if !plain(&transaction)? {
            return Ok(());
        }
        for entry in fs::read_dir(&transaction).map_err(|_| recovery_error())? {
            let entry = entry.map_err(|_| recovery_error())?;
            match entry.file_name().to_str() {
                Some(
                    "new-engine" | "old-engine" | "new-core.toml" | "old-core.toml"
                    | "journal.json",
                ) => {}
                Some(name) if name.starts_with(".rela-") && name.ends_with(".tmp") => {}
                _ => return Err(recovery_error()),
            }
            plain(&entry.path())?;
        }
        remove_engine(&transaction.join("new-engine"))?;
        remove_engine(&transaction.join("old-engine"))?;
        for entry in fs::read_dir(&transaction).map_err(|_| recovery_error())? {
            let entry = entry.map_err(|_| recovery_error())?;
            if entry.file_name() != "journal.json" {
                remove_file(&entry.path())?;
            }
        }
        // Journal is deleted last, so interrupted cleanup can always be retried.
        remove_file(&self.journal_path())?;
        fs::remove_dir(&transaction).map_err(|_| recovery_error())
    }
}

fn upgrade_allowed(old: &Deployment, next: &Deployment) -> Result<(), AppError> {
    if next.owner_app_version < old.owner_app_version
        || next.assets.engine_revision < old.assets.engine_revision
    {
        return Err(AppError::new(
            "core_downgrade_blocked",
            "此 Rela 副本较旧，请使用部署当前引擎的较新版本。",
        ));
    }
    if next.assets.engine_revision == old.assets.engine_revision && next.assets != old.assets {
        return Err(integrity_error());
    }
    Ok(())
}

fn credential_config(bytes: &[u8]) -> bool {
    let Some(text) = std::str::from_utf8(bytes).ok() else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(text) else {
        return false;
    };
    value
        .get("network_identity")
        .and_then(|value| value.get("network_secret"))
        .is_none()
        && value
            .get("secure_mode")
            .and_then(|value| value.get("enabled"))
            .and_then(toml::Value::as_bool)
            == Some(true)
        && value
            .get("secure_mode")
            .and_then(|value| value.get("local_private_key"))
            .and_then(toml::Value::as_str)
            .is_some_and(|secret| {
                use base64::{engine::general_purpose::STANDARD, Engine};
                STANDARD.decode(secret).is_ok_and(|key| key.len() == 32)
            })
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn read(path: &Path, limit: usize) -> Result<Vec<u8>, AppError> {
    if !plain(path)? {
        return Err(recovery_error());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| recovery_error())?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| recovery_error())?;
    if bytes.len() > limit {
        return Err(recovery_error());
    }
    Ok(bytes)
}

pub(super) fn plain(path: &Path) -> Result<bool, AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(recovery_error());
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(recovery_error());
            }
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err(recovery_error()),
    }
}

fn move_path(from: &Path, to: &Path) -> Result<(), AppError> {
    if !plain(from)? || plain(to)? {
        return Err(recovery_error());
    }
    platform::replace_file(from, to).map_err(|_| recovery_error())
}

fn remove_file(path: &Path) -> Result<(), AppError> {
    if plain(path)? {
        if !fs::metadata(path).map_err(|_| recovery_error())?.is_file() {
            return Err(recovery_error());
        }
        fs::remove_file(path).map_err(|_| recovery_error())?;
    }
    Ok(())
}

fn engine_contents(directory: &Path) -> Result<(), AppError> {
    if !plain(directory)? {
        return Err(recovery_error());
    }
    for entry in fs::read_dir(directory).map_err(|_| recovery_error())? {
        let entry = entry.map_err(|_| recovery_error())?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(recovery_error)?;
        if !FILES.contains(&name)
            && name != RECORD
            && !(name.starts_with(".rela-") && name.ends_with(".tmp"))
        {
            return Err(recovery_error());
        }
        plain(&entry.path())?;
        if !entry.file_type().map_err(|_| recovery_error())?.is_file() {
            return Err(recovery_error());
        }
    }
    Ok(())
}

fn remove_engine(path: &Path) -> Result<(), AppError> {
    if !plain(path)? {
        return Ok(());
    }
    engine_contents(path)?;
    for entry in fs::read_dir(path).map_err(|_| recovery_error())? {
        remove_file(&entry.map_err(|_| recovery_error())?.path())?;
    }
    fs::remove_dir(path).map_err(|_| recovery_error())
}

fn recovery_error() -> AppError {
    AppError::new(
        "core_recovery_required",
        "网络引擎切换尚未恢复。已保留备份，请重试或联系管理员，勿删除服务数据。",
    )
}

pub(super) fn validate_application_id(id: &str) -> Result<(), AppError> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(application_pending());
    }
    Ok(())
}
fn application_pending() -> AppError {
    AppError::new(
        "core_application_update_pending",
        "软件更新尚未完成，请使用发起更新的 Rela 副本完成恢复。",
    )
}

#[cfg(test)]
mod tests;
