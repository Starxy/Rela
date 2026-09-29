use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use rela_manifests::{signing_message, PublicKey, SignedEnvelope};

struct Fixture {
    root: PathBuf,
    config: DistributionConfig,
    key: String,
    state: Control,
    zip: Vec<u8>,
    envelope: Vec<u8>,
}
impl Fixture {
    fn new() -> Self {
        let id = ipc::random_id().unwrap();
        let root = std::env::temp_dir().join(format!("rela-controller-test-{id}"));
        let control = root.join(DIRECTORY);
        fs::create_dir_all(&control).unwrap();
        fs::write(root.join("portable.txt"), b"portable").unwrap();
        fs::write(
            root.join("Rela.exe"),
            b"synthetic old application - never executed",
        )
        .unwrap();
        fs::copy(root.join("Rela.exe"), control.join("runner.exe")).unwrap();
        let zip = crate::distribution::portable::tests::fixture_zip("valid");
        let (package, key) = crate::distribution::package::tests::signed(&zip);
        fs::write(control.join("update.zip"), &zip).unwrap();
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
        let mut verified =
            VerifiedPackage::open(&control.join("update.zip"), &package, &key).unwrap();
        portable::stage(&mut verified, &manifest, &control.join("candidate")).unwrap();
        drop(verified);
        let signing = SigningKey::from_bytes(&[53; 32]);
        let mut config = DistributionConfig::bundled().unwrap();
        config.keys = vec![PublicKey {
            id: "controller-test-only".into(),
            purpose: Purpose::Software,
            public_key: STANDARD.encode(signing.verifying_key().to_bytes()),
        }];
        let payload = serde_json::to_vec(&manifest).unwrap();
        let signature = signing
            .sign(&signing_message(Purpose::Software, "controller-test-only", &payload).unwrap());
        let envelope = serde_json::to_vec(&SignedEnvelope {
            format: "rela.signed.v1".into(),
            key_id: "controller-test-only".into(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signature.to_bytes()),
        })
        .unwrap();
        fs::write(control.join("software.json"), &envelope).unwrap();
        let listener = Listener::bind(&id, Role::Controller).unwrap();
        let state = Control {
            schema_version: 1,
            id,
            previous_version: Version::new(0, 1, 0),
            channel: UpdateChannel::Stable,
            envelope_digest: sha256(&envelope),
            runner_digest: file_digest(&control.join("runner.exe"), 1024).unwrap(),
            parent: ProcessTicket {
                pid: 1,
                started_at: "1".into(),
            },
            handoff: listener.endpoint(),
            runner: None,
            candidate: None,
            body: BodyState::Waiting,
            core: CoreState::NotStarted,
            failure: None,
        };
        save(&root, &state).unwrap();
        Self {
            root,
            config,
            key,
            state,
            zip,
            envelope,
        }
    }
    fn verify(&self) -> Result<(VerifiedPackage, SoftwareManifest, PortableStage), AppError> {
        verified_using(&self.root, &self.state, &self.config, &self.key)
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
            .starts_with("rela-controller-test-"));
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn restart_reverifies_signed_feed_zip_and_candidate_before_any_execution() {
    let mut f = Fixture::new();
    let (_, manifest, stage) = f.verify().unwrap();
    assert_eq!(manifest.version, Version::new(0, 2, 0));
    let control = f.root.join(DIRECTORY);
    fs::write(control.join("software.json"), b"different feed").unwrap();
    assert!(f.verify().is_err());
    f.state.envelope_digest = sha256(b"different feed");
    assert!(f.verify().is_err());
    fs::write(control.join("software.json"), &f.envelope).unwrap();
    f.state.envelope_digest = sha256(&f.envelope);
    fs::write(control.join("update.zip"), b"different zip").unwrap();
    assert!(f.verify().is_err());
    fs::write(control.join("update.zip"), &f.zip).unwrap();
    fs::write(stage.directory().join("Rela.exe"), b"different candidate").unwrap();
    assert!(f.verify().is_err());
    assert_eq!(
        fs::read(f.root.join("Rela.exe")).unwrap(),
        b"synthetic old application - never executed"
    );
}

#[test]
fn sealed_control_refuses_corruption_foreign_roles_and_unknown_fields() {
    let mut f = Fixture::new();
    assert_eq!(load(&f.root).unwrap().id, f.state.id);
    let path = f.root.join(DIRECTORY).join(RECORD);
    let sealed = fs::read(&path).unwrap();
    assert!(!sealed
        .windows(f.state.id.len())
        .any(|window| window == f.state.id.as_bytes()));
    fs::write(&path, b"corrupted sealed control").unwrap();
    assert!(load(&f.root).is_err());
    f.state.handoff.role = Role::Core;
    save(&f.root, &f.state).unwrap();
    assert!(load(&f.root).is_err());
    f.state.handoff.role = Role::Controller;
    let mut record = serde_json::to_value(&f.state).unwrap();
    record["target_path"] = "C:/unrelated".into();
    write_private(&path, &record).unwrap();
    assert!(load(&f.root).is_err());
}

#[test]
fn orphaned_body_never_starts_gui_and_unjournaled_download_keeps_original_files() {
    let f = Fixture::new();
    fs::remove_file(f.root.join(DIRECTORY).join(RECORD)).unwrap();
    assert!(!resume_before_gui(&f.root).unwrap());
    fs::create_dir(f.root.join(".rela-update")).unwrap();
    fs::write(f.root.join(".rela-update/keep"), b"unknown recovery file").unwrap();
    assert!(resume_before_gui(&f.root).is_err());
    assert_eq!(
        fs::read(f.root.join(".rela-update/keep")).unwrap(),
        b"unknown recovery file"
    );
}
