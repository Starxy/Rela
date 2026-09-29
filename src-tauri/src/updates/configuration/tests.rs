use super::*;
use crate::updates::ipc;
use std::{
    os::windows::fs::OpenOptionsExt,
    panic::{catch_unwind, AssertUnwindSafe},
};

struct Fixture {
    root: PathBuf,
    config: PathBuf,
    id: String,
    lease: Option<InstanceLease>,
    before: Inventory,
    present: bool,
}
impl Fixture {
    fn new(present: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "rela-config-recovery-test-{}",
            ipc::random_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let config = root.join("configuration");
        if present {
            fs::create_dir(&config).unwrap();
            for name in ["credentials", "cache", "empty"] {
                fs::create_dir(config.join(name)).unwrap();
            }
            fs::write(config.join("preferences.json"), b"old device preferences").unwrap();
            fs::write(
                config.join("credentials/one.dat"),
                platform::protect(b"synthetic credential fixture").unwrap(),
            )
            .unwrap();
            fs::write(
                config.join("cache/resources.json"),
                b"old signed-cache fixture",
            )
            .unwrap();
        }
        fs::write(root.join("unrelated.txt"), b"other application data").unwrap();
        let before = if present {
            inventory(&config).unwrap()
        } else {
            Inventory::default()
        };
        let lease = Some(InstanceLease::acquire(&config).unwrap());
        Self {
            root,
            config,
            id: ipc::random_id().unwrap(),
            lease,
            before,
            present,
        }
    }
    fn lease(&self) -> &InstanceLease {
        self.lease.as_ref().unwrap()
    }
    fn prepare(&self) -> Transaction {
        Transaction::prepare(&self.config, &self.id, self.lease()).unwrap()
    }
    fn reopen(&self) -> Transaction {
        Transaction::reopen(&self.config, &self.id).unwrap()
    }
    fn migrate(&self) {
        if !self.config.exists() {
            fs::create_dir(&self.config).unwrap();
        }
        fs::write(
            self.config.join("preferences.json"),
            b"candidate preferences",
        )
        .unwrap();
        fs::write(
            self.config.join("new-field.json"),
            b"candidate migration data",
        )
        .unwrap();
        if self.config.join("cache/resources.json").exists() {
            fs::remove_file(self.config.join("cache/resources.json")).unwrap();
        }
    }
    fn old(&self) {
        assert_eq!(self.config.exists(), self.present);
        if self.present {
            assert!(inventory(&self.config).unwrap() == self.before);
            assert_eq!(
                platform::unprotect(&fs::read(self.config.join("credentials/one.dat")).unwrap())
                    .unwrap(),
                b"synthetic credential fixture"
            );
        }
        assert_eq!(
            fs::read(self.root.join("unrelated.txt")).unwrap(),
            b"other application data"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.lease.take();
        let root = self.root.canonicalize().unwrap();
        assert_eq!(
            root.parent(),
            Some(std::env::temp_dir().canonicalize().unwrap().as_path())
        );
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rela-config-recovery-test-"));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn snapshot_restores_exact_user_configuration_and_keeps_failed_migration() {
    let f = Fixture::new(true);
    let mut tx = f.prepare();
    f.old();
    f.migrate();
    let candidate = inventory(&f.config).unwrap();
    tx.rollback(f.lease()).unwrap();
    f.old();
    assert!(inventory(&tx.directory().join("failed")).unwrap() == candidate);
    tx.cleanup().unwrap();
    let archived = tx.archive().unwrap();
    assert!(archived.join(JOURNAL).exists());
    assert!(!archived.join("before").exists());
    assert!(inventory(&archived.join("failed")).unwrap() == candidate);
    let mut retry = f.reopen();
    retry.cleanup().unwrap();
    assert_eq!(retry.archive().unwrap(), archived);
    f.old();
}

#[test]
fn every_restore_boundary_survives_failure_or_process_loss() {
    for step in [
        Step::RestoreDecision,
        Step::Displaced,
        Step::Copied,
        Step::Restored,
        Step::RolledBack,
    ] {
        for crash in [false, true] {
            let f = Fixture::new(true);
            let mut tx = f.prepare();
            f.migrate();
            let result = catch_unwind(AssertUnwindSafe(|| {
                tx.rollback_observed(f.lease(), |current| {
                    if current == step {
                        assert!(!crash, "simulated process loss at {step:?}");
                        return Err(error());
                    }
                    Ok(())
                })
            }));
            assert!(result.is_err() || result.unwrap().is_err());
            let mut retry = f.reopen();
            retry.rollback(f.lease()).unwrap();
            retry.cleanup().unwrap();
            retry.archive().unwrap();
            f.old();
        }
    }
}

#[test]
fn missing_original_configuration_is_removed_without_discarding_candidate_data() {
    let f = Fixture::new(false);
    let mut tx = f.prepare();
    f.migrate();
    tx.rollback(f.lease()).unwrap();
    f.old();
    tx.cleanup().unwrap();
    let archived = tx.archive().unwrap();
    assert_eq!(
        fs::read(archived.join("failed/new-field.json")).unwrap(),
        b"candidate migration data"
    );
    f.reopen().rollback(f.lease()).unwrap();
}

#[test]
fn candidate_removing_configuration_and_interrupted_partial_copy_are_recoverable() {
    let f = Fixture::new(true);
    let mut tx = f.prepare();
    // Simulates the candidate atomically moving away its own original tree.
    fs::rename(&f.config, f.root.join("candidate-moved-config")).unwrap();
    assert!(tx
        .rollback_observed(f.lease(), |step| if step == Step::Restored {
            Err(error())
        } else {
            Ok(())
        })
        .is_err());
    f.reopen().rollback(f.lease()).unwrap();
    f.old();

    let f = Fixture::new(true);
    let mut tx = f.prepare();
    f.migrate();
    assert!(tx
        .rollback_observed(f.lease(), |step| if step == Step::Displaced {
            Err(error())
        } else {
            Ok(())
        })
        .is_err());
    let restore = tx.directory().join("restore");
    fs::create_dir(&restore).unwrap();
    fs::write(restore.join("preferences.json"), b"partially copied").unwrap();
    fs::write(restore.join("unrecognized"), b"preserve me").unwrap();
    assert!(f.reopen().rollback(f.lease()).is_err());
    assert_eq!(
        fs::read(restore.join("unrecognized")).unwrap(),
        b"preserve me"
    );
    fs::remove_file(restore.join("unrecognized")).unwrap();
    f.reopen().rollback(f.lease()).unwrap();
    f.old();
}

#[test]
fn commit_preserves_migrated_configuration_and_forbids_later_rollback() {
    let f = Fixture::new(true);
    let mut tx = f.prepare();
    f.migrate();
    let candidate = inventory(&f.config).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(tx.directory().join(JOURNAL))
        .unwrap();
    assert!(tx.commit().is_err());
    assert_eq!(tx.phase(), Phase::Prepared);
    assert!(inventory(&f.config).unwrap() == candidate);
    drop(lock);
    tx.commit().unwrap();
    let mut retry = f.reopen();
    assert_eq!(retry.phase(), Phase::Committed);
    assert!(retry.rollback(f.lease()).is_err());
    retry.commit().unwrap();
    retry.cleanup().unwrap();
    retry.archive().unwrap();
    assert!(inventory(&f.config).unwrap() == candidate);
    assert!(f.reopen().rollback(f.lease()).is_err());
}

#[test]
fn locked_journal_prevents_displacement_and_locked_backup_cleanup_is_retryable() {
    let f = Fixture::new(true);
    let mut tx = f.prepare();
    f.migrate();
    let candidate = inventory(&f.config).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(tx.directory().join(JOURNAL))
        .unwrap();
    assert!(tx.rollback(f.lease()).is_err());
    assert_eq!(tx.phase(), Phase::Prepared);
    assert!(inventory(&f.config).unwrap() == candidate);
    assert!(!tx.directory().join("failed").exists());
    drop(lock);
    tx.rollback(f.lease()).unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(tx.directory().join("before/preferences.json"))
        .unwrap();
    assert!(tx.cleanup().is_err());
    assert!(tx.archive().is_err());
    drop(lock);
    let mut retry = f.reopen();
    retry.cleanup().unwrap();
    retry.archive().unwrap();
    f.old();
}

#[test]
fn wrong_lease_tampered_snapshot_or_journal_never_changes_live_configuration() {
    let f = Fixture::new(true);
    let wrong = InstanceLease::acquire(&f.root.join("other-config")).unwrap();
    assert!(Transaction::prepare(&f.config, &f.id, &wrong).is_err());
    drop(wrong);
    let tx = f.prepare();
    let record = tx.directory().join(JOURNAL);
    let bytes = fs::read(&record).unwrap();
    assert!(!bytes.windows(4).any(|part| part == b"id\":"));
    fs::write(&record, b"broken ciphertext").unwrap();
    assert!(Transaction::reopen(&f.config, &f.id).is_err());
    fs::write(&record, &bytes).unwrap();
    let plaintext = platform::unprotect(&bytes).unwrap();
    for (field, value) in [
        ("config", serde_json::json!(f.root.join("other-config"))),
        ("id", serde_json::json!("f".repeat(64))),
        ("schema_version", serde_json::json!(99)),
        ("extra", serde_json::json!("unknown")),
    ] {
        let mut json: serde_json::Value = serde_json::from_slice(&plaintext).unwrap();
        json[field] = value;
        fs::write(
            &record,
            platform::protect(&serde_json::to_vec(&json).unwrap()).unwrap(),
        )
        .unwrap();
        assert!(Transaction::reopen(&f.config, &f.id).is_err());
    }
    fs::write(record, &bytes).unwrap();
    fs::write(tx.directory().join("before/preferences.json"), b"tampered").unwrap();
    assert!(Transaction::reopen(&f.config, &f.id).is_err());
    f.old();
}

#[test]
fn unknown_content_and_later_user_edits_are_preserved_on_recovery() {
    let f = Fixture::new(true);
    let mut tx = f.prepare();
    f.migrate();
    assert!(tx
        .rollback_observed(f.lease(), |step| if step == Step::Restored {
            Err(error())
        } else {
            Ok(())
        })
        .is_err());
    fs::write(f.config.join("later-user-edit"), b"keep my edit").unwrap();
    assert!(f.reopen().rollback(f.lease()).is_err());
    assert_eq!(
        fs::read(f.config.join("later-user-edit")).unwrap(),
        b"keep my edit"
    );
    fs::remove_file(f.config.join("later-user-edit")).unwrap();
    let mut tx = f.reopen();
    tx.rollback(f.lease()).unwrap();
    fs::write(tx.directory().join("unknown.txt"), b"another file").unwrap();
    assert!(tx.cleanup().is_err());
    assert!(tx.directory().join("before").exists());
    assert_eq!(
        fs::read(tx.directory().join("unknown.txt")).unwrap(),
        b"another file"
    );
    f.old();
}

#[test]
fn completed_id_cannot_be_reused_and_old_receipt_does_not_adopt_new_session() {
    let f = Fixture::new(true);
    let mut tx = f.prepare();
    tx.commit().unwrap();
    tx.cleanup().unwrap();
    tx.archive().unwrap();
    assert!(Transaction::prepare(&f.config, &f.id, f.lease()).is_err());
    let another = ipc::random_id().unwrap();
    let new = Transaction::prepare(&f.config, &another, f.lease()).unwrap();
    assert!(Transaction::reopen(&f.config, &f.id).is_err());
    assert!(new.directory().join("before").exists());
    f.old();
}
