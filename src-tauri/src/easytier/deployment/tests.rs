use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::{AtomicU64, Ordering},
};

const STEPS: [Step; 10] = [
    Step::Prepared,
    Step::Stopped,
    Step::EngineBackedUp,
    Step::ConfigBackedUp,
    Step::EngineReplaced,
    Step::ConfigReplaced,
    Step::Registered,
    Step::Started,
    Step::Published,
    Step::Committed,
];
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn directory(path: &Path) -> Result<(), AppError> {
    fs::create_dir_all(path).map_err(|_| recovery_error())
}
fn file(_: &Path) -> Result<(), AppError> {
    Ok(())
}
fn config(label: &str) -> Vec<u8> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    format!(
        "hostname = '{label}'\n[secure_mode]\nenabled = true\nlocal_private_key = '{}'\n",
        STANDARD.encode([0x42; 32])
    )
    .into_bytes()
}
fn assets(path: &Path, revision: u64, version: &str) -> Deployment {
    fs::create_dir_all(path).unwrap();
    let mut hashes = BTreeMap::new();
    for name in FILES {
        let bytes = format!("synthetic {name} revision {revision}").into_bytes();
        fs::write(path.join(name), &bytes).unwrap();
        hashes.insert(name.to_string(), format!("{:x}", Sha256::digest(&bytes)));
    }
    let deployment = Deployment {
        schema_version: 1,
        owner_app_version: Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
        assets: AssetManifest {
            engine_revision: revision,
            version: version.into(),
            files: hashes,
        },
    };
    fs::write(path.join(RECORD), serde_json::to_vec(&deployment).unwrap()).unwrap();
    deployment
}

struct ControlledService {
    root: PathBuf,
    state: ServiceSnapshot,
    failure: Option<&'static str>,
    starts: usize,
}
impl ControlledService {
    fn fail(&mut self, name: &str) -> Result<(), AppError> {
        if self.failure == Some(name) {
            self.failure = None;
            return Err(recovery_error());
        }
        Ok(())
    }
}
impl Service for ControlledService {
    fn snapshot(&mut self) -> Result<ServiceSnapshot, AppError> {
        Ok(self.state.clone())
    }
    fn stop(&mut self) -> Result<(), AppError> {
        self.fail("stop")?;
        self.state.was_running = false;
        Ok(())
    }
    fn register(&mut self, location: Location) -> Result<(), AppError> {
        self.state.location = Some(location);
        self.fail("register")
    }
    fn start_and_verify(&mut self, version: &str) -> Result<(), AppError> {
        let location = self.state.location.ok_or_else(recovery_error)?;
        let bytes =
            fs::read(location.directory(&self.root).join(RECORD)).map_err(|_| recovery_error())?;
        let deployed: Deployment = serde_json::from_slice(&bytes).unwrap();
        deployed.assets.verify(&location.directory(&self.root))?;
        assert_eq!(deployed.assets.version, version);
        assert!(credential_config(
            &fs::read(self.root.join("core.toml")).unwrap()
        ));
        self.state.was_running = true;
        self.starts += 1;
        self.fail("start")
    }
    fn restore_registration(&mut self, previous: &ServiceSnapshot) -> Result<(), AppError> {
        self.fail("restore")?;
        self.state = previous.clone();
        self.state.was_running = false;
        Ok(())
    }
    fn verify_running(&mut self, version: &str) -> Result<(), AppError> {
        if !self.state.was_running {
            return Err(recovery_error());
        }
        let location = self.state.location.ok_or_else(recovery_error)?;
        let deployed: Deployment = serde_json::from_slice(
            &fs::read(location.directory(&self.root).join(RECORD)).map_err(|_| recovery_error())?,
        )
        .unwrap();
        deployed.assets.verify(&location.directory(&self.root))?;
        if deployed.assets.version != version {
            return Err(recovery_error());
        }
        self.fail("verify")
    }
    fn publish(&mut self, deployment: &PublicDeployment) -> Result<(), AppError> {
        self.state.description = Some(deployment.description()?);
        self.fail("publish")
    }
}

struct Fixture {
    base: PathBuf,
    files: Files,
    service: ControlledService,
    bundled: PathBuf,
    old: Option<Deployment>,
    next: Deployment,
    old_location: Option<Location>,
    old_config: Vec<u8>,
}
impl Fixture {
    fn new(old_location: Option<Location>) -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "rela-core-transaction-{}-{stamp}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let root = base.join("service");
        fs::create_dir_all(&root).unwrap();
        let old = old_location.map(|location| assets(&location.directory(&root), 1, "2.7.0-old"));
        let old_config = config("old-device");
        if old.is_some() {
            fs::write(root.join("core.toml"), &old_config).unwrap();
        }
        let bundled = base.join("bundle");
        let next = assets(&bundled, 2, "2.7.0-new");
        let service = ControlledService {
            root: root.clone(),
            state: ServiceSnapshot {
                location: old_location,
                was_running: old.is_some(),
                description: old.as_ref().map(|old| old.public().description().unwrap()),
            },
            failure: None,
            starts: 0,
        };
        Self {
            base,
            files: Files::new(root, directory, file),
            service,
            bundled,
            old,
            next,
            old_location,
            old_config,
        }
    }
    fn apply(
        &mut self,
        observer: impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        self.files.apply_observed(
            &self.bundled,
            &config("new-device"),
            self.next.clone(),
            &mut self.service,
            observer,
        )
    }
    fn unchanged(&self) {
        assert_eq!(self.service.state.location, self.old_location);
        assert_eq!(self.service.state.was_running, self.old.is_some());
        if let Some(old) = &self.old {
            old.assets
                .verify(&self.old_location.unwrap().directory(&self.files.root))
                .unwrap();
            assert_eq!(fs::read(self.files.config()).unwrap(), self.old_config);
            assert_eq!(
                self.service.state.description,
                Some(old.public().description().unwrap())
            );
        } else {
            assert!(!Location::Managed.directory(&self.files.root).exists());
            assert!(!self.files.config().exists());
        }
        assert!(!self.files.transaction().exists());
    }
    fn updated(&self) {
        self.files.verify_live(&self.next).unwrap();
        assert_eq!(fs::read(self.files.config()).unwrap(), config("new-device"));
        assert_eq!(self.service.state.location, Some(Location::Managed));
        assert!(self.service.state.was_running);
        assert_eq!(
            self.service.state.description,
            Some(self.next.public().description().unwrap())
        );
        assert!(!self.files.transaction().exists());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only remove the unique fixture subtree after resolving its absolute boundary.
        if let (Ok(base), Ok(parent)) = (
            self.base.canonicalize(),
            std::env::temp_dir().canonicalize(),
        ) {
            assert_eq!(base.parent(), Some(parent.as_path()));
            assert!(base
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("rela-core-transaction-"));
            let _ = fs::remove_dir_all(base);
        }
    }
}

#[test]
fn successful_install_update_and_legacy_migration_use_complete_asset_sets() {
    for location in [None, Some(Location::Managed), Some(Location::Legacy)] {
        let mut fixture = Fixture::new(location);
        fixture.apply(|_| Ok(())).unwrap();
        fixture.updated();
        assert!(!Location::Legacy.directory(&fixture.files.root).exists());
    }
}

#[test]
fn every_precommit_failure_restores_files_configuration_registration_and_running_state() {
    for location in [None, Some(Location::Managed), Some(Location::Legacy)] {
        for stop in STEPS.into_iter().filter(|step| *step != Step::Committed) {
            let mut fixture = Fixture::new(location);
            assert!(fixture
                .apply(|step| if step == stop {
                    Err(recovery_error())
                } else {
                    Ok(())
                })
                .is_err());
            fixture.unchanged();
        }
    }
}

#[test]
fn restart_recovers_every_interruption_and_preserves_committed_update() {
    for location in [None, Some(Location::Managed), Some(Location::Legacy)] {
        for stop in STEPS {
            let mut fixture = Fixture::new(location);
            assert!(catch_unwind(AssertUnwindSafe(|| fixture.apply(|step| {
                if step == stop {
                    panic!("simulated process termination");
                }
                Ok(())
            })))
            .is_err());
            fixture.files.recover(&mut fixture.service, true).unwrap();
            if stop == Step::Committed {
                fixture.updated();
            } else {
                fixture.unchanged();
            }
            fixture.files.recover(&mut fixture.service, true).unwrap();
        }
    }
}

#[test]
fn failures_from_service_operations_also_restore_previous_deployment() {
    for operation in ["stop", "register", "start", "publish"] {
        let mut fixture = Fixture::new(Some(Location::Managed));
        fixture.service.failure = Some(operation);
        assert!(fixture.apply(|_| Ok(())).is_err());
        fixture.unchanged();
    }
}

#[test]
fn failed_recovery_keeps_journal_and_can_be_retried() {
    let mut fixture = Fixture::new(Some(Location::Managed));
    fixture.service.failure = Some("restore");
    assert_eq!(
        fixture
            .apply(|step| if step == Step::EngineReplaced {
                Err(recovery_error())
            } else {
                Ok(())
            })
            .unwrap_err()
            .code,
        "core_recovery_required"
    );
    assert!(fixture.files.journal_path().is_file());
    fixture.files.recover(&mut fixture.service, true).unwrap();
    fixture.unchanged();
}

#[test]
fn disconnect_recovery_and_password_migration_never_restart_old_connection() {
    for password in [false, true] {
        let mut fixture = Fixture::new(Some(Location::Legacy));
        if password {
            fs::write(
                fixture.files.config(),
                b"[network_identity]\nnetwork_secret = 'synthetic-password'\n",
            )
            .unwrap();
        }
        assert!(catch_unwind(AssertUnwindSafe(|| fixture.apply(|step| {
            if step == Step::EngineReplaced {
                panic!("termination");
            }
            Ok(())
        })))
        .is_err());
        let result = fixture.files.recover(&mut fixture.service, password);
        if password {
            assert_eq!(result.unwrap_err().code, "credential_migration_required");
        } else {
            result.unwrap();
        }
        assert!(!fixture.service.state.was_running);
        assert_eq!(fixture.service.starts, 0);
        assert!(!fixture.files.transaction().exists());
    }
}

#[test]
fn older_app_engine_and_equal_revision_different_content_are_blocked_before_stop() {
    for case in 0..3 {
        let mut fixture = Fixture::new(Some(Location::Managed));
        match case {
            0 => fixture.next.owner_app_version = Version::new(0, 0, 1),
            1 => fixture.next.assets.engine_revision = 0,
            _ => fixture.next.assets.engine_revision = 1,
        }
        assert!(fixture
            .apply(|_| panic!("must reject before transaction"))
            .is_err());
        fixture.unchanged();
        assert_eq!(fixture.service.starts, 0);
    }
}

#[test]
fn tampered_files_unknown_deployment_and_unexpected_contents_are_preserved() {
    for case in 0..4 {
        let mut fixture = Fixture::new(Some(Location::Managed));
        let directory = Location::Managed.directory(&fixture.files.root);
        match case {
            0 => fs::write(fixture.bundled.join(FILES[0]), b"tampered").unwrap(),
            1 => fs::remove_file(directory.join(RECORD)).unwrap(),
            2 => fs::write(directory.join("unexpected.dll"), b"unknown").unwrap(),
            _ => fs::write(directory.join(FILES[0]), b"tampered").unwrap(),
        }
        assert!(fixture
            .apply(|_| panic!("must reject before transaction"))
            .is_err());
        assert!(fixture.service.state.was_running);
        assert_eq!(
            fs::read(fixture.files.config()).unwrap(),
            fixture.old_config
        );
        assert!(!fixture.files.transaction().exists());
    }
}

#[cfg(windows)]
#[test]
fn locked_backup_after_commit_retains_new_version_and_retries_cleanup() {
    use std::os::windows::fs::OpenOptionsExt;
    let mut fixture = Fixture::new(Some(Location::Managed));
    let backup = fixture
        .files
        .transaction()
        .join("old-engine")
        .join(FILES[0]);
    let mut lock = None;
    assert!(fixture
        .apply(|step| {
            if step == Step::Committed {
                lock = Some(
                    fs::OpenOptions::new()
                        .read(true)
                        .share_mode(0)
                        .open(&backup)
                        .unwrap(),
                );
            }
            Ok(())
        })
        .is_err());
    assert!(fixture.files.journal_path().exists());
    fixture.files.verify_live(&fixture.next).unwrap();
    drop(lock);
    fixture.files.recover(&mut fixture.service, true).unwrap();
    fixture.updated();
}

const APPLICATION_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
impl Fixture {
    fn prepare_application(
        &mut self,
        observer: impl FnMut(Step) -> Result<(), AppError>,
    ) -> Result<bool, AppError> {
        self.files.prepare_application_observed(
            &self.bundled,
            APPLICATION_ID,
            self.next.clone(),
            &mut self.service,
            observer,
        )
    }
    fn resolve(&mut self, decision: Decision) -> Result<(), AppError> {
        self.files
            .resolve_application(APPLICATION_ID, decision, &mut self.service)
    }
}

#[test]
fn application_prepare_waits_for_matching_decision_and_preserves_applied_config() {
    for decision in [Decision::Rollback, Decision::Commit] {
        let mut f = Fixture::new(Some(Location::Managed));
        assert!(f.prepare_application(|_| Ok(())).unwrap());
        f.files.verify_live(&f.next).unwrap();
        assert_eq!(fs::read(f.files.config()).unwrap(), f.old_config);
        assert!(f.files.transaction().join("old-engine").is_dir());
        assert!(f.files.transaction().join("old-core.toml").is_file());
        assert_eq!(
            f.files.recover(&mut f.service, true).unwrap_err().code,
            "core_application_update_pending"
        );
        assert!(f
            .files
            .resolve_application(&"f".repeat(64), decision, &mut f.service)
            .is_err());
        // Retrying prepare validates readiness but neither restarts nor discards backups.
        let starts = f.service.starts;
        assert!(f
            .prepare_application(|_| panic!("must not apply twice"))
            .unwrap());
        assert_eq!(f.service.starts, starts);
        f.resolve(decision).unwrap();
        f.resolve(decision).unwrap();
        assert!(f
            .resolve(if decision == Decision::Commit {
                Decision::Rollback
            } else {
                Decision::Commit
            })
            .is_err());
        assert!(!f.files.transaction().exists());
        if decision == Decision::Rollback {
            f.unchanged();
        } else {
            f.files.verify_live(&f.next).unwrap();
            assert!(f.service.state.was_running);
            assert_eq!(fs::read(f.files.config()).unwrap(), f.old_config);
        }
    }
}

#[test]
fn stopped_and_absent_services_are_not_started_by_an_application_update() {
    let mut absent = Fixture::new(None);
    assert!(!absent
        .prepare_application(|_| panic!("no service transaction"))
        .unwrap());
    absent.unchanged();
    assert_eq!(absent.service.starts, 0);
    for decision in [Decision::Rollback, Decision::Commit] {
        let mut f = Fixture::new(Some(Location::Managed));
        f.service.state.was_running = false;
        assert!(f.prepare_application(|_| Ok(())).unwrap());
        assert!(!f.service.state.was_running);
        f.resolve(decision).unwrap();
        assert!(!f.service.state.was_running);
        assert_eq!(f.service.starts, 0);
        assert_eq!(fs::read(f.files.config()).unwrap(), f.old_config);
    }
}

#[test]
fn lost_application_helper_keeps_backups_until_explicit_rollback_at_every_step() {
    for point in STEPS
        .into_iter()
        .filter(|step| *step != Step::Committed)
        .chain([Step::AwaitingApplication])
    {
        let mut f = Fixture::new(Some(Location::Managed));
        assert!(
            catch_unwind(AssertUnwindSafe(|| f.prepare_application(|step| {
                if step == point {
                    panic!("lost helper");
                }
                Ok(())
            })))
            .is_err()
        );
        assert_eq!(
            f.files.recover(&mut f.service, true).unwrap_err().code,
            "core_application_update_pending"
        );
        if point != Step::AwaitingApplication {
            assert!(f.resolve(Decision::Commit).is_err());
        }
        f.resolve(Decision::Rollback).unwrap();
        f.unchanged();
        assert_eq!(
            f.files.receipt(APPLICATION_ID).unwrap(),
            Some(Decision::Rollback)
        );
    }
}

#[test]
fn application_prepare_errors_rollback_and_record_a_retryable_receipt() {
    for point in [
        Step::Prepared,
        Step::EngineReplaced,
        Step::Started,
        Step::AwaitingApplication,
    ] {
        let mut f = Fixture::new(Some(Location::Managed));
        assert!(f
            .prepare_application(|step| if step == point {
                Err(recovery_error())
            } else {
                Ok(())
            })
            .is_err());
        f.unchanged();
        f.resolve(Decision::Rollback).unwrap();
        assert!(f.prepare_application(|_| Ok(())).is_err());
    }
}

#[test]
fn application_commit_revalidates_config_and_running_core() {
    for case in ["config", "core", "rpc"] {
        let mut f = Fixture::new(Some(Location::Managed));
        f.prepare_application(|_| Ok(())).unwrap();
        match case {
            "config" => fs::write(f.files.config(), b"tampered config").unwrap(),
            "core" => fs::write(
                Location::Managed.directory(&f.files.root).join(FILES[0]),
                b"tampered binary",
            )
            .unwrap(),
            _ => f.service.failure = Some("verify"),
        }
        assert!(f.resolve(Decision::Commit).is_err());
        assert!(f.files.journal_path().is_file());
        f.resolve(Decision::Rollback).unwrap();
        f.unchanged();
    }
}

#[cfg(windows)]
#[test]
fn committed_application_cleanup_never_changes_to_rollback_on_retry() {
    use std::os::windows::fs::OpenOptionsExt;
    let mut f = Fixture::new(Some(Location::Managed));
    f.prepare_application(|_| Ok(())).unwrap();
    // Permit reads so validation succeeds, but deny delete so cleanup fails.
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(f.files.transaction().join("old-engine").join(FILES[0]))
        .unwrap();
    assert!(f.resolve(Decision::Commit).is_err());
    assert_eq!(
        f.files.receipt(APPLICATION_ID).unwrap(),
        Some(Decision::Commit)
    );
    assert!(f.resolve(Decision::Rollback).is_err());
    f.files.verify_live(&f.next).unwrap();
    drop(lock);
    f.resolve(Decision::Commit).unwrap();
    f.resolve(Decision::Commit).unwrap();
    assert!(!f.files.transaction().exists());
}
