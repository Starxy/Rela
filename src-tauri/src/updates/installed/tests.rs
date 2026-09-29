// Real body files with in-memory Windows metadata; no native system writes.
use super::*;
use crate::distribution::{
    package::{tests::signed, VerifiedPackage},
    portable::{stage, tests::fixture_zip},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rela_manifests::{Channel, Package, SoftwareManifest};
use std::{
    os::windows::fs::OpenOptionsExt,
    panic::{catch_unwind, AssertUnwindSafe},
    time::{SystemTime, UNIX_EPOCH},
};

pub(in crate::updates) const ID: &str =
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

// This fixture never opens HKLM, launches a program, elevates, or touches SCM.
// Only the body files are real; system metadata is an in-memory model.
pub(in crate::updates) struct FakeSystem {
    root: PathBuf,
    state: Snapshot,
    fail_field: Option<Field>,
}
impl SystemState for FakeSystem {
    fn validate_root(&self, root: &Path) -> Result<(), AppError> {
        plain(root, true)?;
        if root.canonicalize().map_err(|_| pending())? != self.root {
            return Err(pending());
        }
        Ok(())
    }
    fn validate_live_file(&self, path: &Path) -> Result<(), AppError> {
        if !path.starts_with(&self.root) {
            return Err(pending());
        }
        plain(path, false)
    }
    fn create_session(&self, path: &Path) -> Result<(), AppError> {
        if path != self.root.join(DIRECTORY) {
            return Err(pending());
        }
        fs::create_dir(path).map_err(|_| pending())
    }
    fn validate_session(&self, path: &Path) -> Result<(), AppError> {
        if path != self.root.join(DIRECTORY) {
            return Err(pending());
        }
        plain(path, true)
    }
    fn snapshot(&self) -> Result<Snapshot, AppError> {
        Ok(self.state.clone())
    }
    fn restore_registry(&mut self, field: Field, value: Option<&Value>) -> Result<(), AppError> {
        self.state.registry.insert(field, value.cloned());
        if self.fail_field == Some(field) {
            return Err(pending());
        }
        Ok(())
    }
    fn restore_shortcut(
        &mut self,
        shortcut: Shortcut,
        value: Option<&str>,
    ) -> Result<(), AppError> {
        self.state
            .shortcuts
            .insert(shortcut, value.map(str::to_owned));
        Ok(())
    }
    fn publish_restored_file(&self, path: &Path) -> Result<(), AppError> {
        self.validate_live_file(path)
    }
}

pub(in crate::updates) struct Fixture {
    pub(in crate::updates) root: PathBuf,
    pub(in crate::updates) stage: PortableStage,
    pub(in crate::updates) system: FakeSystem,
    before: Snapshot,
}
impl Fixture {
    pub(in crate::updates) fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rela-installed-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        for directory in ["easytier", "third-party-licenses", "data"] {
            fs::create_dir(root.join(directory)).unwrap();
        }
        for name in backup_names() {
            fs::write(root.join(name), format!("MZ original {name}")).unwrap();
        }
        // A missing optional old slot must be removed during rollback.
        fs::remove_file(root.join("Remove-Network-Service.cmd")).unwrap();
        fs::write(root.join("README.txt"), b"not an installed payload slot").unwrap();
        fs::write(root.join("mine.txt"), b"unrelated user file").unwrap();
        fs::write(
            root.join("data/credential.dat"),
            b"synthetic encrypted data",
        )
        .unwrap();

        let mut registry: BTreeMap<_, _> = Field::ALL.into_iter().map(|id| (id, None)).collect();
        for (field, value) in [
            (Field::InstallRoot, root.to_str().unwrap().to_owned()),
            (Field::MainBinaryName, "rela.exe".into()),
            (Field::DisplayName, "Rela".into()),
            (
                Field::DisplayIcon,
                format!("\"{}\\rela.exe\"", root.display()),
            ),
            (Field::DisplayVersion, "0.1.0".into()),
            (Field::Publisher, "teamsillybees".into()),
            (Field::InstallLocation, format!("\"{}\"", root.display())),
            (
                Field::UninstallString,
                format!("\"{}\\uninstall.exe\"", root.display()),
            ),
        ] {
            registry.insert(field, Some(Value::String(value)));
        }
        for field in [Field::NoModify, Field::NoRepair, Field::EstimatedSize] {
            registry.insert(field, Some(Value::Dword(1)));
        }
        let before = Snapshot {
            registry,
            shortcuts: Shortcut::ALL
                .into_iter()
                .map(|id| (id, Some(STANDARD.encode(format!("old {id:?} shortcut")))))
                .collect(),
        };
        before
            .validate_installation(&root, &Version::new(0, 1, 0))
            .unwrap();
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
        let system = FakeSystem {
            root: root.clone(),
            state: before.clone(),
            fail_field: None,
        };
        Self {
            root,
            stage,
            system,
            before,
        }
    }
    pub(in crate::updates) fn prepare(&self) -> Transaction {
        Transaction::prepare(
            &self.root,
            &self.stage,
            Version::new(0, 1, 0),
            ID,
            &self.system,
        )
        .unwrap()
    }
    pub(in crate::updates) fn reopen(&self) -> Transaction {
        Transaction::reopen(&self.root, &self.stage, ID, &self.system).unwrap()
    }
    pub(in crate::updates) fn nsis_writes(&mut self) {
        for name in program_names() {
            fs::copy(self.stage.directory().join(name), self.root.join(name)).unwrap();
        }
        fs::write(
            self.root.join("uninstall.exe"),
            b"MZ synthetic new uninstaller",
        )
        .unwrap();
        self.system
            .state
            .registry
            .insert(Field::DisplayVersion, Some(Value::String("0.2.0".into())));
        self.system
            .state
            .registry
            .insert(Field::EstimatedSize, Some(Value::Dword(1024)));
        self.system.state.registry.insert(
            Field::HelpLink,
            Some(Value::String("https://example.invalid/help".into())),
        );
        for id in Shortcut::ALL {
            self.system
                .state
                .shortcuts
                .insert(id, Some(STANDARD.encode(format!("new {id:?} shortcut"))));
        }
    }
    pub(in crate::updates) fn preserved(&self) {
        assert_eq!(
            fs::read(self.root.join("README.txt")).unwrap(),
            b"not an installed payload slot"
        );
        assert_eq!(
            fs::read(self.root.join("mine.txt")).unwrap(),
            b"unrelated user file"
        );
        assert_eq!(
            fs::read(self.root.join("data/credential.dat")).unwrap(),
            b"synthetic encrypted data"
        );
        assert!(!self.root.join("portable.txt").exists());
    }
    pub(in crate::updates) fn old(&self) {
        for name in backup_names() {
            if name == "Remove-Network-Service.cmd" {
                assert!(!self.root.join(name).exists());
            } else {
                assert_eq!(
                    fs::read(self.root.join(name)).unwrap(),
                    format!("MZ original {name}").as_bytes()
                );
            }
        }
        assert_eq!(self.system.state, self.before);
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
            .starts_with("rela-installed-test-"));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn preparation_and_partial_installer_failure_preserve_old_installation_and_user_files() {
    let mut f = Fixture::new();
    let mut tx = f.prepare();
    f.old();
    fs::write(f.root.join("Rela.exe"), b"partial installer output").unwrap();
    fs::write(f.root.join("uninstall.exe"), b"truncated").unwrap();
    f.system
        .state
        .registry
        .insert(Field::DisplayName, Some(Value::String("partial".into())));
    assert!(tx.installed(&f.system).is_err());
    assert!(tx.begin_commit(&f.system).is_err());
    tx.rollback(&mut f.system).unwrap();
    f.old();
    tx.cleanup(&f.system).unwrap();
    f.reopen().cleanup(&f.system).unwrap();
    assert!(!tx.directory().exists());
}

#[test]
fn each_rollback_boundary_survives_errors_and_process_loss() {
    let mut f = Fixture::new();
    let mut tx = f.prepare();
    f.nsis_writes();
    tx.installed(&f.system).unwrap();
    let mut steps = Vec::new();
    tx.rollback_observed(&mut f.system, |step| {
        steps.push(step);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        steps.len(),
        backup_names().count() + Field::ALL.len() + Shortcut::ALL.len() + 1
    );
    for failure in steps {
        for crash in [false, true] {
            let mut f = Fixture::new();
            let mut tx = f.prepare();
            f.nsis_writes();
            tx.installed(&f.system).unwrap();
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                tx.rollback_observed(&mut f.system, |step| {
                    if step == failure {
                        assert!(!crash, "simulated process loss at {step:?}");
                        return Err(pending());
                    }
                    Ok(())
                })
            }));
            assert!(outcome.is_err() || outcome.unwrap().is_err());
            let mut resumed = f.reopen();
            resumed.rollback(&mut f.system).unwrap();
            f.old();
            resumed.cleanup(&f.system).unwrap();
            f.reopen().rollback(&mut f.system).unwrap();
        }
    }
}

#[test]
fn registry_write_failure_and_locked_live_file_leave_retryable_backups() {
    let mut f = Fixture::new();
    let mut tx = f.prepare();
    f.nsis_writes();
    tx.installed(&f.system).unwrap();
    f.system.fail_field = Some(Field::InstallLocation);
    assert!(tx.rollback(&mut f.system).is_err());
    assert!(tx.directory().join("old/uninstall.exe").exists());
    f.system.fail_field = None;
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(f.root.join("Rela.exe"))
        .unwrap();
    assert!(f.reopen().rollback(&mut f.system).is_err());
    drop(lock);
    f.reopen().rollback(&mut f.system).unwrap();
    f.old();
}

#[test]
fn commit_decision_is_durable_and_cannot_revert_after_core_may_have_committed() {
    let mut f = Fixture::new();
    let mut tx = f.prepare();
    f.nsis_writes();
    tx.installed(&f.system).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(tx.directory().join("journal.json"))
        .unwrap();
    assert!(tx.begin_commit(&f.system).is_err());
    assert_eq!(tx.phase(), Phase::Installed);
    drop(lock);
    let mut tx = f.reopen();
    tx.begin_commit(&f.system).unwrap();
    let mut tx = f.reopen();
    assert_eq!(tx.phase(), Phase::Committing);
    assert!(tx.rollback(&mut f.system).is_err());
    assert!(tx.cleanup(&f.system).is_err());
    tx.finish_commit(&f.system).unwrap();
    tx.cleanup(&f.system).unwrap();
    // Lost final acknowledgement and repeated completion both remain safe.
    let mut tx = f.reopen();
    assert_eq!(tx.phase(), Phase::Committed);
    tx.finish_commit(&f.system).unwrap();
    tx.cleanup(&f.system).unwrap();
    assert!(tx.rollback(&mut f.system).is_err());
    assert!(Transaction::prepare(&f.root, &f.stage, Version::new(0, 1, 0), ID, &f.system).is_err());
    assert_eq!(
        fs::read(f.root.join("Rela.exe")).unwrap(),
        fs::read(f.stage.directory().join("Rela.exe")).unwrap()
    );
    f.preserved();
}

#[test]
fn cleanup_retries_locked_backups_and_survives_missing_journal_acknowledgement() {
    let mut f = Fixture::new();
    let mut tx = f.prepare();
    tx.rollback(&mut f.system).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(tx.directory().join("old/uninstall.exe"))
        .unwrap();
    assert!(tx.cleanup(&f.system).is_err());
    assert!(receipt_path(&f.root, ID).exists());
    assert!(tx.directory().join("journal.json").exists());
    drop(lock);
    f.reopen().cleanup(&f.system).unwrap();
    // Simulate a crash between removing the journal and removing its directory.
    fs::create_dir(tx.directory()).unwrap();
    f.reopen().cleanup(&f.system).unwrap();
    f.reopen().cleanup(&f.system).unwrap();
    f.old();
}

#[test]
fn unrecognized_cleanup_content_and_a_new_unjournaled_session_are_preserved() {
    let mut f = Fixture::new();
    let mut tx = f.prepare();
    tx.rollback(&mut f.system).unwrap();
    let unknown = tx.directory().join("old/keep.txt");
    fs::write(&unknown, b"unrecognized").unwrap();
    assert!(tx.cleanup(&f.system).is_err());
    assert_eq!(fs::read(&unknown).unwrap(), b"unrecognized");
    fs::remove_file(unknown).unwrap();
    tx.cleanup(&f.system).unwrap();
    fs::create_dir(tx.directory()).unwrap();
    fs::write(
        tx.directory().join("journal.pending"),
        b"another preparation",
    )
    .unwrap();
    assert!(tx.cleanup(&f.system).is_err());
    assert!(Transaction::reopen(&f.root, &f.stage, ID, &f.system).is_err());
    assert_eq!(
        fs::read(tx.directory().join("journal.pending")).unwrap(),
        b"another preparation"
    );
    f.old();
}

#[test]
fn tampered_backup_candidate_journal_and_receipt_are_rejected() {
    for target in ["old/Rela.exe", "next/Rela.exe", "journal.json"] {
        let f = Fixture::new();
        let tx = f.prepare();
        fs::write(tx.directory().join(target), b"tampered").unwrap();
        assert!(Transaction::reopen(&f.root, &f.stage, ID, &f.system).is_err());
        f.old();
    }
    let mut f = Fixture::new();
    let mut tx = f.prepare();
    tx.rollback(&mut f.system).unwrap();
    tx.cleanup(&f.system).unwrap();
    let receipt = receipt_path(&f.root, ID);
    let original = fs::read(&receipt).unwrap();
    for (field, value) in [
        ("phase", serde_json::json!("prepared")),
        ("id", serde_json::json!("f".repeat(64))),
        ("schema_version", serde_json::json!(20)),
        ("root", serde_json::json!(f.root.join("elsewhere"))),
        ("arbitrary_key", serde_json::json!("HKLM/other")),
    ] {
        let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
        record[field] = value;
        fs::write(&receipt, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(
            Transaction::reopen(&f.root, &f.stage, ID, &f.system).is_err(),
            "accepted {field}"
        );
        assert!(tx.cleanup(&f.system).is_err());
    }
    fs::write(receipt, original).unwrap();
    f.reopen().cleanup(&f.system).unwrap();
}

#[test]
fn receipt_conflicting_with_existing_journal_never_discards_backups() {
    let mut f = Fixture::new();
    let mut tx = f.prepare();
    tx.rollback(&mut f.system).unwrap();
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(tx.directory().join("journal.json")).unwrap()).unwrap();
    receipt["before"]["registry"]["estimated_size"] =
        serde_json::json!({"type":"dword","value":99});
    fs::write(
        receipt_path(&f.root, ID),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    assert!(Transaction::reopen(&f.root, &f.stage, ID, &f.system).is_err());
    assert!(tx.cleanup(&f.system).is_err());
    assert!(tx.directory().join("old/Rela.exe").exists());
    f.old();
}

#[test]
fn occupied_program_and_wrong_registered_installation_fail_before_any_snapshot() {
    let mut f = Fixture::new();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(f.root.join("Rela.exe"))
        .unwrap();
    assert!(Transaction::prepare(&f.root, &f.stage, Version::new(0, 1, 0), ID, &f.system).is_err());
    assert!(!f.root.join(DIRECTORY).exists());
    drop(lock);
    f.system.state.registry.insert(
        Field::MainBinaryName,
        Some(Value::String("another.exe".into())),
    );
    assert!(Transaction::prepare(&f.root, &f.stage, Version::new(0, 1, 0), ID, &f.system).is_err());
    assert!(!f.root.join(DIRECTORY).exists());
}

#[test]
fn metadata_rejects_extra_arguments_wrong_types_missing_fields_and_unbounded_values() {
    let f = Fixture::new();
    for (field, value) in [
        (
            Field::UninstallString,
            Some(Value::String(format!(
                "\"{}\\uninstall.exe\" /other",
                f.root.display()
            ))),
        ),
        (
            Field::MainBinaryName,
            Some(Value::String("../elsewhere.exe".into())),
        ),
        (
            Field::InstallLocation,
            Some(Value::String("\"C:\\wrong\"".into())),
        ),
        (Field::NoModify, Some(Value::String("1".into()))),
        (Field::HelpLink, Some(Value::String("x".repeat(2048)))),
        (
            Field::UrlInfoAbout,
            Some(Value::String("bad\0suffix".into())),
        ),
        (Field::DisplayName, None),
    ] {
        let mut snapshot = f.before.clone();
        snapshot.registry.insert(field, value);
        assert!(snapshot
            .validate_installation(&f.root, &Version::new(0, 1, 0))
            .is_err());
    }
    let mut snapshot = f.before.clone();
    snapshot.registry.remove(&Field::HelpLink);
    assert!(snapshot.validate().is_err());
    for bytes in [
        "not base64!".into(),
        STANDARD.encode(vec![0u8; 256 * 1024 + 1]),
    ] {
        let mut snapshot = f.before.clone();
        snapshot.shortcuts.insert(Shortcut::Desktop, Some(bytes));
        assert!(snapshot.validate().is_err());
    }
    let mut value = serde_json::to_value(&f.before).unwrap();
    value["registry"]["arbitrary_registry_key"] = serde_json::json!(null);
    assert!(serde_json::from_value::<Snapshot>(value).is_err());
}
