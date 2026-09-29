use super::*;
use crate::updates::{installed::tests::Fixture, ipc::Listener};
use std::{fs, os::windows::fs::OpenOptionsExt};

#[derive(Default)]
struct FakeCore {
    committed: bool,
    rolled_back: bool,
    calls: usize,
    fail_before: bool,
    lose_acknowledgement: bool,
}
impl Core for FakeCore {
    fn prepare(&mut self, _: &Path) -> Result<bool, AppError> {
        unreachable!("the resolution tests never install a real service")
    }
    fn commit(&mut self) -> Result<(), AppError> {
        self.calls += 1;
        if self.fail_before || self.rolled_back {
            return Err(error());
        }
        self.committed = true;
        if self.lose_acknowledgement {
            Err(error())
        } else {
            Ok(())
        }
    }
    fn rollback(&mut self) -> Result<(), AppError> {
        self.calls += 1;
        if self.fail_before || self.committed {
            return Err(error());
        }
        self.rolled_back = true;
        if self.lose_acknowledgement {
            Err(error())
        } else {
            Ok(())
        }
    }
}

#[test]
fn commit_retries_core_failure_and_lost_ack_but_never_reverses_durable_body_decision() {
    for lose_ack in [false, true] {
        let mut f = Fixture::new();
        let mut body = f.prepare();
        f.nsis_writes();
        body.installed(&f.system).unwrap();
        let mut core = FakeCore {
            fail_before: !lose_ack,
            lose_acknowledgement: lose_ack,
            ..Default::default()
        };
        assert!(resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::Prepared,
            Command::Commit
        )
        .is_err());
        let mut body = f.reopen();
        assert_eq!(body.phase(), Phase::Committing);
        assert!(body.directory().join("old/Rela.exe").is_file());
        assert!(resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::Prepared,
            Command::Rollback
        )
        .is_err());
        assert_eq!(core.calls, 1);
        core.fail_before = false;
        core.lose_acknowledgement = false;
        let state = resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::Prepared,
            Command::Commit,
        )
        .unwrap();
        assert_eq!(state, CoreState::Committed);
        assert_eq!(body.phase(), Phase::Committed);
        assert!(may_finalize(body.phase(), state));
        body.cleanup(&f.system).unwrap();
        let mut body = f.reopen();
        // Both participant acknowledgement and final controller acknowledgement
        // may be lost after durable receipts have been written.
        resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::Prepared,
            Command::Commit,
        )
        .unwrap();
        body.cleanup(&f.system).unwrap();
        f.preserved();
        assert_eq!(
            fs::read(f.root.join("Rela.exe")).unwrap(),
            fs::read(f.stage.directory().join("Rela.exe")).unwrap()
        );
    }
}

#[test]
fn failed_body_decision_does_not_commit_core_and_can_still_roll_back() {
    let mut f = Fixture::new();
    let mut body = f.prepare();
    f.nsis_writes();
    body.installed(&f.system).unwrap();
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(body.directory().join("journal.json"))
        .unwrap();
    let mut core = FakeCore::default();
    assert!(resolve(
        &mut body,
        &mut f.system,
        &mut core,
        CoreState::Prepared,
        Command::Commit
    )
    .is_err());
    assert_eq!(body.phase(), Phase::Installed);
    assert_eq!(core.calls, 0);
    drop(file);
    resolve(
        &mut body,
        &mut f.system,
        &mut core,
        CoreState::Prepared,
        Command::Rollback,
    )
    .unwrap();
    assert!(core.rolled_back);
    f.old();
}

#[test]
fn rollback_retries_core_failure_after_body_restoration_and_lost_ack() {
    for lose_ack in [false, true] {
        let mut f = Fixture::new();
        let mut body = f.prepare();
        // Partial NSIS writes still have no installed-body acknowledgement.
        fs::write(f.root.join("Rela.exe"), b"partial NSIS output").unwrap();
        let mut core = FakeCore {
            fail_before: !lose_ack,
            lose_acknowledgement: lose_ack,
            ..Default::default()
        };
        assert!(resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::Requested,
            Command::Rollback
        )
        .is_err());
        f.old();
        let mut body = f.reopen();
        assert_eq!(body.phase(), Phase::RolledBack);
        assert!(!may_finalize(body.phase(), CoreState::Requested));
        core.fail_before = false;
        core.lose_acknowledgement = false;
        let state = resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::Requested,
            Command::Rollback,
        )
        .unwrap();
        assert_eq!(state, CoreState::RolledBack);
        assert!(may_finalize(body.phase(), state));
        body.cleanup(&f.system).unwrap();
        let mut body = f.reopen();
        resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::RolledBack,
            Command::Rollback,
        )
        .unwrap();
        f.old();
    }
}

#[test]
fn commit_requires_completed_core_preparation_and_rollback_never_reverses_core_commit() {
    for state in [
        CoreState::NotStarted,
        CoreState::Requested,
        CoreState::RolledBack,
        CoreState::Committed,
    ] {
        let mut f = Fixture::new();
        let mut body = f.prepare();
        f.nsis_writes();
        body.installed(&f.system).unwrap();
        let mut core = FakeCore::default();
        assert!(resolve(&mut body, &mut f.system, &mut core, state, Command::Commit).is_err());
        assert_eq!(body.phase(), Phase::Installed);
        assert_eq!(core.calls, 0);
        if state == CoreState::Committed {
            assert!(resolve(
                &mut body,
                &mut f.system,
                &mut core,
                state,
                Command::Rollback
            )
            .is_err());
            assert_eq!(body.phase(), Phase::Installed);
        }
    }
    let mut f = Fixture::new();
    let mut body = f.prepare();
    let mut core = FakeCore::default();
    for command in [
        Command::Commit,
        Command::Prepare,
        Command::AllowInstall,
        Command::Finalize,
    ] {
        assert!(resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::Prepared,
            command
        )
        .is_err());
    }
    assert_eq!(core.calls, 0);
    f.old();
}

#[test]
fn no_service_update_never_opens_core_and_only_matching_terminal_states_finalize() {
    let mut f = Fixture::new();
    let mut body = f.prepare();
    f.nsis_writes();
    body.installed(&f.system).unwrap();
    let mut core = FakeCore::default();
    assert_eq!(
        resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::NotNeeded,
            Command::Commit
        )
        .unwrap(),
        CoreState::NotNeeded
    );
    assert_eq!(core.calls, 0);
    for body in [
        Phase::Prepared,
        Phase::Installed,
        Phase::Committing,
        Phase::Committed,
        Phase::RolledBack,
    ] {
        for core in [
            CoreState::NotStarted,
            CoreState::Requested,
            CoreState::NotNeeded,
            CoreState::Prepared,
            CoreState::Committed,
            CoreState::RolledBack,
        ] {
            let expected = (body == Phase::Committed
                && [CoreState::NotNeeded, CoreState::Committed].contains(&core))
                || (body == Phase::RolledBack
                    && [
                        CoreState::NotStarted,
                        CoreState::NotNeeded,
                        CoreState::RolledBack,
                    ]
                    .contains(&core));
            assert_eq!(may_finalize(body, core), expected);
        }
    }
}

fn request(root: &Path, id: &str) -> Request {
    Request {
        schema_version: 1,
        id: id.into(),
        root: root.into(),
        previous_version: Version::new(0, 1, 0),
        channel: UpdateChannel::Stable,
        envelope_digest: "a".repeat(64),
        runner_digest: "b".repeat(64),
        controller: ProcessTicket {
            pid: 1,
            started_at: "1".into(),
        },
        pre: Listener::bind(id, Role::InstallerPre).unwrap().endpoint(),
        post: Listener::bind(id, Role::InstallerPost).unwrap().endpoint(),
        recovery: None,
    }
}

#[test]
fn sealed_request_binds_paths_selection_process_and_separate_hook_roles() {
    let f = Fixture::new();
    let id = ipc::random_id().unwrap();
    let control = f.root.join(format!(".rela-installed-user-{id}"));
    fs::create_dir(&control).unwrap();
    let record = request(&f.root, &id);
    write_request(&control, &record).unwrap();
    assert!(read_request(&control.join(REQUEST_FILE)).unwrap() == record);
    let bytes = fs::read(control.join(REQUEST_FILE)).unwrap();
    assert!(!bytes.windows(id.len()).any(|value| value == id.as_bytes()));
    let mut recovery = record.clone();
    recovery.recovery = Some(
        Listener::bind(&id, Role::InstallerRecovery)
            .unwrap()
            .endpoint(),
    );
    recovery.controller.pid = 2;
    recovery.validate().unwrap();
    assert!(recovery.same_selection(&record));
    recovery.root = f.root.join("foreign");
    assert!(!recovery.same_selection(&record));
    for field in [
        "role",
        "id",
        "digest",
        "pid",
        "time",
        "version",
        "relative_root",
    ] {
        let mut invalid = record.clone();
        match field {
            "role" => invalid.pre.role = Role::InstallerPost,
            "id" => invalid.post.id = "c".repeat(64),
            "digest" => invalid.runner_digest = "c".repeat(63),
            "pid" => invalid.controller.pid = 0,
            "time" => invalid.controller.started_at = "invalid".into(),
            "version" => invalid.previous_version = Version::parse("0.1.0+foreign").unwrap(),
            _ => invalid.root = "relative".into(),
        }
        assert!(
            write_request(&control, &invalid).is_err(),
            "accepted {field}"
        );
    }
    fs::write(control.join(REQUEST_FILE), b"tampered").unwrap();
    assert!(read_request(&control.join(REQUEST_FILE)).is_err());
    let mut unknown = serde_json::to_value(&record).unwrap();
    unknown["arbitrary_command"] = "ignored?".into();
    fs::write(
        control.join(REQUEST_FILE),
        platform::protect_request(&serde_json::to_vec(&unknown).unwrap()).unwrap(),
    )
    .unwrap();
    assert!(read_request(&control.join(REQUEST_FILE)).is_err());
    assert!(write_request(&f.root, &record).is_err());
}

#[test]
fn lost_final_ack_reopens_archive_but_cannot_bypass_new_active_transaction() {
    let f = Fixture::new();
    let id = ipc::random_id().unwrap();
    let archived = archive_location(&f.root, &id);
    fs::create_dir(&archived).unwrap();
    assert_eq!(control_location(&f.root, &id).unwrap(), archived);
    let active = f.root.join(guard::CONTROL);
    fs::create_dir(&active).unwrap();
    assert_eq!(control_location(&f.root, &id).unwrap(), active);
    assert!(control_location(&f.root, "../foreign").is_err());
}

fn record(f: &Fixture) -> Record {
    let mut request = request(&f.root, crate::updates::installed::tests::ID);
    request.runner_digest = fingerprint(&f.root.join("Rela.exe"), MAX_PROGRAM)
        .unwrap()
        .sha256;
    Record {
        request,
        schema_version: 1,
        installer: ProcessTicket {
            pid: 1,
            started_at: "1".into(),
        },
        installer_image: f.root.join("synthetic-never-executed.exe"),
        authorized: false,
        core: CoreState::NotStarted,
    }
}

#[test]
fn interrupted_preinstall_rebuilds_only_unstarted_backups_and_retains_partial_content() {
    for partial in [false, true] {
        let mut f = Fixture::new();
        let record = record(&f);
        if partial {
            let body = f.prepare();
            fs::remove_file(body.directory().join("journal.json")).unwrap();
            fs::write(
                body.directory().join("unknown-retained.txt"),
                b"retain partial preparation",
            )
            .unwrap();
        }
        let mut body = reopen_body(&f.root, &f.stage, &record, &f.system, true).unwrap();
        f.old();
        let mut core = FakeCore::default();
        resolve(
            &mut body,
            &mut f.system,
            &mut core,
            CoreState::NotStarted,
            Command::Rollback,
        )
        .unwrap();
        assert_eq!(core.calls, 0);
        body.cleanup(&f.system).unwrap();
        f.old();
        let retained: Vec<_> = fs::read_dir(&f.root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".rela-installed-preparation-")
            })
            .collect();
        assert_eq!(retained.len(), usize::from(partial));
        if partial {
            assert_eq!(
                fs::read(retained[0].path().join("unknown-retained.txt")).unwrap(),
                b"retain partial preparation"
            );
        }
    }
}

#[test]
fn authorized_or_corrupted_body_is_never_rebuilt_as_unstarted_preparation() {
    for condition in [
        "authorized",
        "core_started",
        "changed_live",
        "corrupted_journal",
        "post_mode",
    ] {
        let f = Fixture::new();
        let mut record = record(&f);
        let body = f.prepare();
        fs::remove_file(body.directory().join("journal.json")).unwrap();
        match condition {
            "authorized" => record.authorized = true,
            "core_started" => record.core = CoreState::Requested,
            "changed_live" => {
                fs::write(f.root.join("Rela.exe"), b"not original").unwrap();
            }
            "corrupted_journal" => {
                fs::write(body.directory().join("journal.json"), b"corrupt").unwrap();
            }
            _ => {}
        }
        assert!(
            reopen_body(
                &f.root,
                &f.stage,
                &record,
                &f.system,
                condition != "post_mode"
            )
            .is_err(),
            "accepted {condition}"
        );
        assert!(body.directory().join("old/Rela.exe").is_file());
    }
}

#[test]
fn source_reverification_binds_signed_selection_package_and_every_candidate_byte() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use rela_manifests::{signing_message, PublicKey, SignedEnvelope};
    let f = Fixture::new();
    let id = ipc::random_id().unwrap();
    let control = f.root.join(format!(".rela-installed-user-{id}"));
    fs::create_dir(&control).unwrap();
    let bytes = crate::distribution::portable::tests::fixture_zip("valid");
    let (package, key) = crate::distribution::package::tests::signed(&bytes);
    fs::write(control.join("update.zip"), &bytes).unwrap();
    let manifest = SoftwareManifest {
        schema_version: 1,
        revision: 1,
        version: Version::new(0, 2, 0),
        channel: UpdateChannel::Stable,
        target: "windows-x86_64".into(),
        notes: "synthetic test".into(),
        published_at: "2026-09-29T00:00:00Z".into(),
        core_version: "2.7.0-test".into(),
        minimum_app_version: Version::new(0, 1, 0),
        installer: rela_manifests::Package {
            url: package.url.replace(".zip", ".exe"),
            ..package.clone()
        },
        portable: package.clone(),
    };
    let mut verified = VerifiedPackage::open(&control.join("update.zip"), &package, &key).unwrap();
    portable::stage(&mut verified, &manifest, &control.join("candidate")).unwrap();
    drop(verified);
    let signing = SigningKey::from_bytes(&[61; 32]);
    let mut config = DistributionConfig::bundled().unwrap();
    config.keys = vec![PublicKey {
        id: "worker-test-only".into(),
        purpose: Purpose::Software,
        public_key: STANDARD.encode(signing.verifying_key().to_bytes()),
    }];
    let payload = serde_json::to_vec(&manifest).unwrap();
    let envelope = serde_json::to_vec(&SignedEnvelope {
        format: "rela.signed.v1".into(),
        key_id: "worker-test-only".into(),
        payload: STANDARD.encode(&payload),
        signature: STANDARD.encode(
            signing
                .sign(&signing_message(Purpose::Software, "worker-test-only", &payload).unwrap())
                .to_bytes(),
        ),
    })
    .unwrap();
    fs::write(control.join("software.json"), &envelope).unwrap();
    let mut request = request(&f.root, &id);
    request.envelope_digest = sha256(&envelope);
    let source = verify_source_using(&control, &request, &config, &key).unwrap();
    assert_eq!(source.manifest.version, Version::new(0, 2, 0));
    assert!(fs::write(
        control.join("update.zip"),
        b"cannot replace authenticated package"
    )
    .is_err());
    drop(source);
    let mut rejected = request.clone();
    rejected.channel = UpdateChannel::Test;
    assert!(verify_source_using(&control, &rejected, &config, &key).is_err());
    rejected = request.clone();
    rejected.previous_version = Version::new(0, 2, 0);
    assert!(verify_source_using(&control, &rejected, &config, &key).is_err());
    fs::write(control.join("software.json"), b"tampered").unwrap();
    assert!(verify_source_using(&control, &request, &config, &key).is_err());
    rejected.envelope_digest = sha256(b"tampered");
    assert!(verify_source_using(&control, &rejected, &config, &key).is_err());
    fs::write(control.join("software.json"), envelope).unwrap();
    fs::write(control.join("update.zip"), b"wrong package").unwrap();
    assert!(verify_source_using(&control, &request, &config, &key).is_err());
    fs::write(control.join("update.zip"), bytes).unwrap();
    fs::write(control.join("candidate/Rela.exe"), b"changed candidate").unwrap();
    assert!(verify_source_using(&control, &request, &config, &key).is_err());
    f.old();
}
