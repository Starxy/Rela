use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use rela_manifests::{
    signing_message, Channel as UpdateChannel, Package, PublicKey, Purpose, SignedEnvelope,
    SoftwareManifest,
};
use std::io::Cursor;

// Synthetic, non-executable PE headers only. No installation, elevation, GUI,
// registry or service access. All signed keys are generated in this fixture.
struct Fixture {
    directory: PathBuf,
    root: PathBuf,
    control: PathBuf,
    state: Control,
    distribution: DistributionConfig,
    key: String,
    zip: Vec<u8>,
    installer: Vec<u8>,
    envelope: Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        let id = ipc::random_id().unwrap();
        let directory = std::env::temp_dir().join(format!("rela-installed-controller-test-{id}"));
        fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let root = directory.join("installed");
        fs::create_dir(&root).unwrap();
        fs::create_dir(directory.join("profile")).unwrap();
        let config = directory.join("profile/config");
        fs::create_dir(&config).unwrap();
        fs::write(
            config.join("credential.dat"),
            platform::protect(b"synthetic credential").unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("Rela.exe"),
            b"MZ old synthetic image, never executed",
        )
        .unwrap();
        let control = control_location(&config, &id).unwrap();
        platform::create_private_directory(&control).unwrap();
        copy_runner(&root.join("Rela.exe"), &control.join("runner.exe")).unwrap();
        let zip = crate::distribution::portable::tests::fixture_zip("valid");
        let installer = b"MZ signed synthetic installer, never executed".to_vec();
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let key = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
        let sign = |bytes: &[u8], filename: &str| Package {
            url: format!("https://github.com/Starxy/Rela/releases/download/v0.2.0/{filename}"),
            size: bytes.len() as u64,
            sha256: sha256(bytes),
            signature: STANDARD.encode(
                minisign::sign(
                    Some(&pair.pk),
                    &pair.sk,
                    Cursor::new(bytes),
                    Some("timestamp:1\tversion:0.2.0"),
                    None,
                )
                .unwrap()
                .to_string(),
            ),
        };
        let manifest = SoftwareManifest {
            schema_version: 1,
            revision: 1,
            version: Version::new(0, 2, 0),
            channel: UpdateChannel::Stable,
            target: "windows-x86_64".into(),
            notes: "fixture".into(),
            published_at: "2026-09-29T00:00:00Z".into(),
            core_version: "2.7.0-test".into(),
            minimum_app_version: Version::new(0, 1, 0),
            installer: sign(&installer, "setup.exe"),
            portable: sign(&zip, "portable.zip"),
        };
        fs::write(control.join("update.zip"), &zip).unwrap();
        fs::write(control.join("installer.exe"), &installer).unwrap();
        let mut package =
            VerifiedPackage::open(&control.join("update.zip"), &manifest.portable, &key).unwrap();
        portable::stage(&mut package, &manifest, &control.join("candidate")).unwrap();
        drop(package);
        let signing = SigningKey::from_bytes(&[74; 32]);
        let mut distribution = DistributionConfig::bundled().unwrap();
        distribution.keys = vec![PublicKey {
            id: "installed-controller-test".into(),
            purpose: Purpose::Software,
            public_key: STANDARD.encode(signing.verifying_key().to_bytes()),
        }];
        let payload = serde_json::to_vec(&manifest).unwrap();
        let signature = signing
            .sign(&signing_message(Purpose::Software, &distribution.keys[0].id, &payload).unwrap());
        let envelope = serde_json::to_vec(&SignedEnvelope {
            format: "rela.signed.v1".into(),
            key_id: distribution.keys[0].id.clone(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signature.to_bytes()),
        })
        .unwrap();
        fs::write(control.join("software.json"), &envelope).unwrap();
        let parent = ProcessTicket {
            pid: 1,
            started_at: "1".into(),
        };
        let state = Control {
            schema_version: 1,
            request: Request {
                schema_version: 1,
                id: id.clone(),
                root: root.clone(),
                previous_version: Version::new(0, 1, 0),
                channel: UpdateChannel::Stable,
                envelope_digest: sha256(&envelope),
                runner_digest: fingerprint(&root.join("Rela.exe"), MAX_PROGRAM)
                    .unwrap()
                    .sha256,
                controller: parent.clone(),
                pre: Listener::bind(&id, Role::InstallerPre).unwrap().endpoint(),
                post: Listener::bind(&id, Role::InstallerPost).unwrap().endpoint(),
                recovery: None,
            },
            config,
            parent,
            handoff: Listener::bind(&id, Role::Controller).unwrap().endpoint(),
            runner: None,
            launcher: None,
            launcher_endpoint: None,
            hook: None,
            installer: None,
            installer_image: None,
            candidate: None,
            phase: Phase::Waiting,
            machine_done: false,
            failure: None,
        };
        save(&control, &state).unwrap();
        publish_pointer(&control, &state).unwrap();
        Self {
            directory,
            root,
            control,
            state,
            distribution,
            key,
            zip,
            installer,
            envelope,
        }
    }
    fn verify(&self) -> Result<(VerifiedSource, VerifiedPackage, File), AppError> {
        verify_using(&self.control, &self.state, &self.distribution, &self.key)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let path = self.directory.canonicalize().unwrap();
        assert_eq!(
            path.parent(),
            Some(std::env::temp_dir().canonicalize().unwrap().as_path())
        );
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rela-installed-controller-test-"));
        fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn reopened_source_reverifies_both_packages_manifest_and_runner_and_holds_their_bytes() {
    let mut f = Fixture::new();
    let verified = f.verify().unwrap();
    for name in ["runner.exe", "update.zip", "installer.exe"] {
        assert!(fs::write(f.control.join(name), b"changed").is_err());
        assert!(fs::remove_file(f.control.join(name)).is_err());
    }
    drop(verified);
    for (name, original) in [
        ("installer.exe", f.installer.clone()),
        ("update.zip", f.zip.clone()),
        ("software.json", f.envelope.clone()),
    ] {
        fs::write(f.control.join(name), b"tampered").unwrap();
        assert!(f.verify().is_err(), "accepted {name}");
        fs::write(f.control.join(name), original).unwrap();
    }
    f.state.request.channel = UpdateChannel::Test;
    assert!(f.verify().is_err());
    f.state.request.channel = UpdateChannel::Stable;
    fs::write(f.control.join("runner.exe"), b"wrong old application").unwrap();
    assert!(f.verify().is_err());
    assert_eq!(
        fs::read(f.root.join("Rela.exe")).unwrap(),
        b"MZ old synthetic image, never executed"
    );
}

#[test]
fn pointer_binds_original_profile_and_installation_and_survives_configuration_rename() {
    let f = Fixture::new();
    let path = pointer_location(&f.state.config).unwrap();
    let sealed = fs::read(&path).unwrap();
    assert!(!sealed
        .windows(f.state.request.id.len())
        .any(|s| s == f.state.request.id.as_bytes()));
    assert!(pointed(&f.state.config, &f.root).unwrap().is_some());
    let moved = f.state.config.with_file_name("saved-config");
    fs::rename(&f.state.config, &moved).unwrap();
    fs::create_dir(&f.state.config).unwrap();
    assert_eq!(pointer_location(&f.state.config).unwrap(), path);
    assert!(pointed(&f.state.config, &f.root).unwrap().is_some());
    assert!(pointed(&f.state.config, &f.directory).is_err());
    let foreign = f.directory.join("profile/foreign");
    fs::copy(&path, pointer_location(&foreign).unwrap()).unwrap();
    assert!(pointed(&foreign, &f.root).is_err());
    fs::write(&path, b"corrupt pointer").unwrap();
    assert!(pointed(&f.state.config, &f.root).is_err());
    assert!(f.control.join("runner.exe").is_file());
    assert!(moved.join("credential.dat").is_file());
}

#[test]
fn sealed_control_rejects_unknown_fields_invalid_role_tickets_and_foreign_location() {
    let mut f = Fixture::new();
    f.state.handoff.role = Role::Core;
    assert!(save(&f.control, &f.state).is_err());
    f.state.handoff.role = Role::Controller;
    f.state.candidate = Some(ProcessTicket {
        pid: 0,
        started_at: "1".into(),
    });
    assert!(save(&f.control, &f.state).is_err());
    f.state.candidate = None;
    let mut unknown = serde_json::to_value(&f.state).unwrap();
    unknown["executable"] = "C:/foreign.exe".into();
    write_private(&f.control.join(RECORD), &unknown).unwrap();
    assert!(load(&f.control).is_err());
    save(&f.control, &f.state).unwrap();
    let foreign = f.control.with_file_name(format!(
        ".rela-installed-user-{}",
        ipc::random_id().unwrap()
    ));
    fs::create_dir(&foreign).unwrap();
    fs::copy(f.control.join(RECORD), foreign.join(RECORD)).unwrap();
    assert!(load(&foreign).is_err());
}

#[test]
fn archived_control_requires_terminal_state_and_never_hides_active_corruption() {
    let mut f = Fixture::new();
    let archive = archive_location(&f.control).unwrap();
    fs::rename(&f.control, &archive).unwrap();
    assert!(pointed(&f.state.config, &f.root).is_err());
    f.state.phase = Phase::Committed;
    write_private(&archive.join(RECORD), &f.state).unwrap();
    assert!(pointed(&f.state.config, &f.root).is_err());
    f.state.machine_done = true;
    write_private(&archive.join(RECORD), &f.state).unwrap();
    assert_eq!(
        pointed(&f.state.config, &f.root).unwrap().unwrap().0,
        archive
    );
    fs::create_dir(&f.control).unwrap();
    fs::write(f.control.join(RECORD), b"new incomplete transaction").unwrap();
    assert!(pointed(&f.state.config, &f.root).is_err());
    assert!(archive.join("runner.exe").is_file());
}

#[test]
fn live_guard_requires_common_signed_files_and_excludes_portable_only_files() {
    let f = Fixture::new();
    let (source, _, _) = f.verify().unwrap();
    for name in installed::program_names() {
        let destination = f.root.join(name);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::copy(source.stage.directory().join(name), destination).unwrap();
    }
    let guard = lock_program(&f.root, source.stage.index()).unwrap();
    assert!(!f.root.join("checksums.json").exists());
    assert!(!f.root.join("README.txt").exists());
    assert!(fs::write(f.root.join("Rela.exe"), b"tampered").is_err());
    assert!(fs::remove_file(f.root.join("easytier/wintun.dll")).is_err());
    drop(guard);
    fs::write(f.root.join("easytier/wintun.dll"), b"tampered").unwrap();
    assert!(lock_program(&f.root, source.stage.index()).is_err());
}

#[test]
fn configuration_resolution_retries_lost_final_ack_and_cannot_reverse_machine_commit() {
    use super::super::{configuration, installed::Phase as BodyPhase};
    for commit in [false, true] {
        let f = Fixture::new();
        let lease = InstanceLease::acquire(&f.state.config).unwrap();
        let original = fs::read(f.state.config.join("credential.dat")).unwrap();
        let mut config = Some(
            configuration::Transaction::prepare(&f.state.config, &f.state.request.id, &lease)
                .unwrap(),
        );
        fs::write(
            f.state.config.join("credential.dat"),
            platform::protect(b"candidate synthetic credential").unwrap(),
        )
        .unwrap();
        let migrated = fs::read(f.state.config.join("credential.dat")).unwrap();
        let confirmed = if commit {
            BodyPhase::Committed
        } else {
            BodyPhase::RolledBack
        };
        assert!(
            runtime::finish_configuration(&mut config, Some(&lease), confirmed, |_| Err(error()))
                .is_err()
        );
        let transaction =
            configuration::Transaction::reopen(&f.state.config, &f.state.request.id).unwrap();
        assert_eq!(
            transaction.phase(),
            if commit {
                configuration::Phase::Committed
            } else {
                configuration::Phase::RolledBack
            }
        );
        config = Some(transaction);
        runtime::finish_configuration(&mut config, Some(&lease), confirmed, |phase| {
            assert_eq!(phase, confirmed);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::read(f.state.config.join("credential.dat")).unwrap(),
            if commit { migrated } else { original }
        );
        let opposite = if commit {
            BodyPhase::RolledBack
        } else {
            BodyPhase::Committed
        };
        let mut finalized = false;
        assert!(
            runtime::finish_configuration(&mut config, Some(&lease), opposite, |_| {
                finalized = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!finalized);
    }
}

#[test]
fn failed_configuration_checks_never_finalize_machine_or_discard_its_backup() {
    use super::super::{configuration, installed::Phase as BodyPhase};
    let f = Fixture::new();
    let lease = InstanceLease::acquire(&f.state.config).unwrap();
    let transaction =
        configuration::Transaction::prepare(&f.state.config, &f.state.request.id, &lease).unwrap();
    let snapshot = transaction.directory().join("before/credential.dat");
    fs::write(&snapshot, b"corrupted snapshot").unwrap();
    let mut config = Some(transaction);
    for phase in [
        BodyPhase::Prepared,
        BodyPhase::Installed,
        BodyPhase::Committing,
        BodyPhase::Committed,
        BodyPhase::RolledBack,
    ] {
        let mut finalized = false;
        assert!(
            runtime::finish_configuration(&mut config, Some(&lease), phase, |_| {
                finalized = true;
                Ok(())
            })
            .is_err()
        );
        assert!(!finalized);
        assert!(snapshot.is_file());
    }
}
