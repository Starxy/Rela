use super::*;
use crate::distribution::{
    package::{tests::signed, VerifiedPackage},
    portable::{stage, tests::fixture_zip},
};
use rela_manifests::{Channel, Package, SoftwareManifest};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    root: PathBuf,
    stage: PortableStage,
    before: Inventory,
}
const TRANSACTION_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
impl Fixture {
    fn new(with_config: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "rela-replacement-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        for directory in ["easytier", "third-party-licenses", "data", "data/logs"] {
            fs::create_dir(root.join(directory)).unwrap();
        }
        for name in UPDATE_FILES {
            fs::write(root.join(name), format!("original {name}")).unwrap();
        }
        fs::write(root.join("portable.txt"), b"marker unchanged").unwrap();
        fs::write(root.join("my-file.txt"), b"unrelated file unchanged").unwrap();
        fs::write(root.join("data/logs/keep.txt"), b"logs unchanged").unwrap();
        if with_config {
            for directory in [
                "data/config",
                "data/config/credentials",
                "data/config/cache",
                "data/config/empty",
            ] {
                fs::create_dir(root.join(directory)).unwrap();
            }
            fs::write(
                root.join("data/config/configuration.json"),
                b"synthetic previous configuration",
            )
            .unwrap();
            fs::write(
                root.join("data/config/credentials/test.dat"),
                b"synthetic DPAPI blob, no real secret",
            )
            .unwrap();
            fs::write(
                root.join("data/config/cache/resources.json"),
                b"synthetic old cache",
            )
            .unwrap();
        }
        let before = if with_config {
            inventory(&root.join("data/config")).unwrap()
        } else {
            Inventory::default()
        };
        let bytes = fixture_zip("valid");
        let (package, key) = signed(&bytes);
        let manifest = SoftwareManifest {
            schema_version: 1,
            revision: 1,
            version: Version::new(0, 2, 0),
            channel: Channel::Stable,
            target: "windows-x86_64".into(),
            notes: "test".into(),
            published_at: "2026-09-29T00:00:00Z".into(),
            core_version: "2.7.0-test".into(),
            minimum_app_version: Version::new(0, 1, 0),
            installer: Package {
                url: package.url.replace(".zip", ".exe"),
                ..package.clone()
            },
            portable: package.clone(),
        };
        fs::write(root.join("package.zip"), bytes).unwrap();
        let mut package = VerifiedPackage::open(&root.join("package.zip"), &package, &key).unwrap();
        let stage = stage(&mut package, &manifest, &root.join("verified")).unwrap();
        Self {
            root,
            stage,
            before,
        }
    }
    fn prepare(&self) -> Transaction {
        Transaction::prepare(
            &self.root,
            &self.stage,
            Version::new(0, 1, 0),
            TRANSACTION_ID,
        )
        .unwrap()
    }
    fn reopen(&self) -> Transaction {
        Transaction::reopen(&self.root, &self.stage, TRANSACTION_ID).unwrap()
    }
    fn preserved(&self) {
        assert_eq!(
            fs::read(self.root.join("portable.txt")).unwrap(),
            b"marker unchanged"
        );
        assert_eq!(
            fs::read(self.root.join("my-file.txt")).unwrap(),
            b"unrelated file unchanged"
        );
        assert_eq!(
            fs::read(self.root.join("data/logs/keep.txt")).unwrap(),
            b"logs unchanged"
        );
    }
    fn old(&self, with_config: bool) {
        for name in UPDATE_FILES {
            assert_eq!(
                fs::read(self.root.join(name)).unwrap(),
                format!("original {name}").as_bytes()
            );
        }
        assert!(!self.root.join("checksums.json").exists());
        assert_eq!(self.root.join("data/config").exists(), with_config);
        if with_config {
            assert!(inventory(&self.root.join("data/config")).unwrap() == self.before);
        }
        self.preserved();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let root = self.root.canonicalize().unwrap();
        assert_eq!(
            root.parent(),
            Some(std::env::temp_dir().canonicalize().unwrap().as_path())
        );
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rela-replacement-"));
        fs::remove_dir_all(root).unwrap();
    }
}

struct Participant {
    root: PathBuf,
    fail: Option<&'static str>,
    prepared: bool,
    committed: bool,
    rolled_back: bool,
    launched: bool,
}
impl Participant {
    fn new(f: &Fixture) -> Self {
        Self {
            root: f.root.clone(),
            fail: None,
            prepared: false,
            committed: false,
            rolled_back: false,
            launched: false,
        }
    }
    fn check(&mut self, name: &'static str) -> Result<(), AppError> {
        if self.fail == Some(name) {
            self.fail = None;
            Err(AppError::new("injected_failure", name))
        } else {
            Ok(())
        }
    }
}
impl Runtime for Participant {
    fn prepare(&mut self) -> Result<(), AppError> {
        self.prepared = true;
        self.check("prepare")
    }
    fn quiesce(&mut self) -> Result<(), AppError> {
        self.check("quiesce")
    }
    fn start_and_verify(&mut self, executable: &Path) -> Result<(), AppError> {
        assert_eq!(
            executable,
            self.root.canonicalize().unwrap().join("Rela.exe")
        );
        self.launched = true;
        fs::create_dir_all(self.root.join("data/config/new-schema")).unwrap();
        fs::write(
            self.root.join("data/config/configuration.json"),
            b"candidate migrated configuration",
        )
        .unwrap();
        fs::write(
            self.root.join("data/config/new-schema/keep.json"),
            b"new data retained for diagnosis",
        )
        .unwrap();
        self.check("start")
    }
    fn rollback(&mut self) -> Result<(), AppError> {
        self.check("rollback")?;
        self.rolled_back = true;
        Ok(())
    }
    fn commit(&mut self) -> Result<(), AppError> {
        self.check("commit")?;
        self.committed = true;
        Ok(())
    }
}
fn steps() -> Vec<Step> {
    [Step::Prepared, Step::CorePrepared]
        .into_iter()
        .chain((0..UPDATE_FILES.len() + 1).map(Step::Replaced))
        .chain([Step::Started, Step::Committing, Step::Committed])
        .collect()
}

#[test]
fn successful_transaction_preserves_data_and_commits_after_startup() {
    let f = Fixture::new(true);
    let mut t = f.prepare();
    let mut p = Participant::new(&f);
    t.execute(&mut p).unwrap();
    assert_eq!(t.phase(), Phase::Committed);
    assert!(p.prepared && p.launched && p.committed && !p.rolled_back);
    t.verify_live_next().unwrap();
    assert!(inventory(&t.session.join("config-before")).unwrap() == f.before);
    assert_eq!(
        fs::read(f.root.join("data/config/configuration.json")).unwrap(),
        b"candidate migrated configuration"
    );
    f.reopen().recover(&mut p).unwrap();
    f.preserved();
}

#[test]
fn every_failed_step_rolls_back_before_decision_and_forward_after_it() {
    for with_config in [true, false] {
        for point in steps() {
            let f = Fixture::new(with_config);
            let mut t = f.prepare();
            let mut p = Participant::new(&f);
            assert!(t
                .execute_observed(&mut p, |step| if step == point {
                    Err(AppError::new("injected", "file boundary"))
                } else {
                    Ok(())
                })
                .is_err());
            let mut t = f.reopen();
            t.recover(&mut p).unwrap();
            if matches!(point, Step::Committing | Step::Committed) {
                assert_eq!(t.phase(), Phase::Committed);
                t.verify_live_next().unwrap();
                assert!(!p.rolled_back);
            } else {
                assert_eq!(t.phase(), Phase::RolledBack);
                f.old(with_config);
            }
        }
    }
}

#[test]
fn process_loss_at_every_file_boundary_recovers_from_disk() {
    for point in steps() {
        let f = Fixture::new(true);
        let mut t = f.prepare();
        let mut p = Participant::new(&f);
        assert!(catch_unwind(AssertUnwindSafe(|| {
            let _ = t.execute_observed(&mut p, |step| {
                if step == point {
                    panic!("simulated process loss");
                }
                Ok(())
            });
        }))
        .is_err());
        drop(t);
        let mut recovered = f.reopen();
        recovered.recover(&mut p).unwrap();
        if matches!(point, Step::Committing | Step::Committed) {
            assert_eq!(recovered.phase(), Phase::Committed);
            recovered.verify_live_next().unwrap();
        } else {
            assert_eq!(recovered.phase(), Phase::RolledBack);
            f.old(true);
        }
    }
}

#[test]
fn interrupted_rollback_is_repeatable_and_candidate_configuration_is_retained() {
    let points: Vec<_> = (0..UPDATE_FILES.len() + 1)
        .map(Step::Restored)
        .chain([
            Step::ConfigDisplaced,
            Step::ConfigRestored,
            Step::RolledBack,
        ])
        .collect();
    for point in points {
        let f = Fixture::new(true);
        let mut t = f.prepare();
        let mut p = Participant::new(&f);
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _ = t.execute_observed(&mut p, |step| {
                if step == Step::Started {
                    panic!("simulated process loss");
                }
                Ok(())
            });
        }));
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _ = t.recover_observed(&mut p, |step| {
                if step == point {
                    panic!("simulated recovery loss");
                }
                Ok(())
            });
        }));
        f.reopen().recover(&mut p).unwrap();
        f.old(true);
        assert_eq!(
            fs::read(t.session.join("config-failed/new-schema/keep.json")).unwrap(),
            b"new data retained for diagnosis"
        );
    }
}

#[test]
fn participant_failure_and_commit_retry_respect_the_durable_decision() {
    for operation in ["prepare", "start", "commit"] {
        let f = Fixture::new(true);
        let mut t = f.prepare();
        let mut p = Participant::new(&f);
        p.fail = Some(operation);
        assert!(t.execute(&mut p).is_err());
        f.reopen().recover(&mut p).unwrap();
        if operation == "commit" {
            assert!(!p.rolled_back);
            t.verify_live_next().unwrap();
        } else {
            f.old(true);
        }
    }
}

#[test]
fn tampering_with_stage_backup_or_journal_is_rejected_without_launch() {
    for case in ["next", "old", "config-before", "journal"] {
        let f = Fixture::new(true);
        let t = f.prepare();
        let path = match case {
            "next" => t.session.join("next/Rela.exe"),
            "old" => t.session.join("old/Rela.exe"),
            "config-before" => t.session.join("config-before/configuration.json"),
            _ => t.session.join("journal.json"),
        };
        fs::write(path, b"tampered").unwrap();
        assert!(Transaction::reopen(&f.root, &f.stage, TRANSACTION_ID).is_err());
        f.old(true);
    }
    let f = Fixture::new(true);
    fs::write(
        f.stage.directory().join("Rela.exe"),
        b"tampered after signature verification",
    )
    .unwrap();
    assert!(
        Transaction::prepare(&f.root, &f.stage, Version::new(0, 1, 0), TRANSACTION_ID).is_err()
    );
    f.old(true);
}

#[test]
fn partial_config_copy_can_resume_without_deleting_an_unrecognized_file() {
    let f = Fixture::new(true);
    let t = f.prepare();
    let restore = t.session.join("config-restore");
    fs::create_dir(&restore).unwrap();
    fs::write(restore.join("configuration.json"), b"partial").unwrap();
    copy_inventory(
        &t.session.join("config-before"),
        &restore,
        &t.journal.config,
    )
    .unwrap();
    assert!(inventory(&restore).unwrap() == f.before);
    fs::write(restore.join("unrecognized"), b"keep").unwrap();
    assert!(copy_inventory(
        &t.session.join("config-before"),
        &restore,
        &t.journal.config
    )
    .is_err());
    assert_eq!(fs::read(restore.join("unrecognized")).unwrap(), b"keep");
}

#[cfg(windows)]
#[test]
fn locked_destination_retains_backups_and_recovers_when_file_is_released() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new(true);
    let mut t = f.prepare();
    let mut p = Participant::new(&f);
    let locked = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(f.root.join("Rela.exe"))
        .unwrap();
    assert!(t.execute(&mut p).is_err());
    assert_eq!(t.phase(), Phase::Prepared);
    assert!(!p.launched && !p.committed);
    drop(locked);
    f.reopen().recover(&mut p).unwrap();
    f.old(true);
}

#[test]
fn cleanup_requires_a_terminal_decision_and_preserves_failed_migration_data() {
    for committed in [false, true] {
        let f = Fixture::new(true);
        let mut t = f.prepare();
        assert!(t.cleanup().is_err());
        let mut p = Participant::new(&f);
        if committed {
            t.execute(&mut p).unwrap();
        } else {
            p.fail = Some("start");
            assert!(t.execute(&mut p).is_err());
        }
        t.cleanup().unwrap();
        t.cleanup().unwrap();
        assert!(!t.session.exists());
        if committed {
            t.verify_live_next().unwrap();
        } else {
            f.old(true);
            assert_eq!(
                fs::read(f.root.join(format!(
                    "data/recovery/update-{TRANSACTION_ID}/new-schema/keep.json"
                )))
                .unwrap(),
                b"new data retained for diagnosis"
            );
        }
        f.preserved();
    }
}

#[cfg(windows)]
#[test]
fn cleanup_interruption_after_backup_removal_reopens_without_rolling_back() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new(true);
    let mut t = f.prepare();
    let mut p = Participant::new(&f);
    t.execute(&mut p).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(t.session.join("next/Rela.exe"))
        .unwrap();
    assert!(t.cleanup().is_err());
    assert!(!t.session.join("old").exists());
    assert!(t.session.join("journal.json").is_file());
    let mut reopened = f.reopen();
    reopened.recover(&mut p).unwrap();
    assert_eq!(reopened.phase(), Phase::Committed);
    assert!(!p.rolled_back);
    drop(lock);
    reopened.cleanup().unwrap();
    assert!(!t.session.exists());
}

#[test]
fn foreign_transaction_and_unknown_backup_file_are_rejected_and_preserved() {
    let f = Fixture::new(true);
    let mut t = f.prepare();
    assert!(Transaction::reopen(&f.root, &f.stage, &"f".repeat(64)).is_err());
    let mut p = Participant::new(&f);
    t.execute(&mut p).unwrap();
    fs::write(t.session.join("old/unrecognized.txt"), b"preserve").unwrap();
    assert!(t.cleanup().is_err());
    assert_eq!(
        fs::read(t.session.join("old/unrecognized.txt")).unwrap(),
        b"preserve"
    );
    t.verify_live_next().unwrap();
}
