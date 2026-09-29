#![cfg(windows)]

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use rela_lib::updates::installer::{InstallerPlan, OfficialInstaller};
use rela_manifests::{sha256, signing_message, Package, SignedEnvelope};
use rela_manifests::{Channel, PublicKey, Purpose, SoftwareManifest};
use rela_protocol::AppError;
use semver::Version;
use std::{fs, io::Cursor, path::PathBuf};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

struct Fixture {
    directory: PathBuf,
    artifact: Vec<u8>,
    public_key: String,
    keys: Vec<PublicKey>,
    signing: SigningKey,
    manifest: SoftwareManifest,
}

impl Fixture {
    fn new(comment: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "rela-installer-{}",
            rela_lib::updates::ipc::random_id().unwrap()
        ));
        fs::create_dir(&directory).unwrap();
        // Intentionally non-executable, synthetic bytes. No test calls launch().
        let artifact = b"MZ Rela installer adapter fixture; not an executable".to_vec();
        fs::write(directory.join("setup.exe"), &artifact).unwrap();
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let public_key = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
        let signature = minisign::sign(
            Some(&pair.pk),
            &pair.sk,
            Cursor::new(&artifact),
            Some(comment),
            None,
        )
        .unwrap();
        let installer = Package {
            url: "https://github.com/Starxy/Rela/releases/download/v0.2.0/Rela-setup.exe".into(),
            size: artifact.len() as u64,
            sha256: sha256(&artifact),
            signature: STANDARD.encode(signature.to_string()),
        };
        let mut portable = installer.clone();
        portable.url =
            "https://github.com/Starxy/Rela/releases/download/v0.2.0/Rela-update.zip".into();
        let signing = SigningKey::from_bytes(&[91; 32]);
        let keys = vec![PublicKey {
            id: "installer-test".into(),
            purpose: Purpose::Software,
            public_key: STANDARD.encode(signing.verifying_key().as_bytes()),
        }];
        let manifest = SoftwareManifest {
            schema_version: 1,
            revision: 1,
            version: Version::parse("0.2.0").unwrap(),
            channel: Channel::Stable,
            target: "windows-x86_64".into(),
            notes: "安装版支持性测试".into(),
            published_at: "2026-09-29T00:00:00Z".into(),
            core_version: "2.7.0-0a783c8e".into(),
            minimum_app_version: Version::parse("0.1.0").unwrap(),
            installer,
            portable,
        };
        Self {
            directory,
            artifact,
            public_key,
            keys,
            signing,
            manifest,
        }
    }

    fn envelope(&self) -> Vec<u8> {
        let payload = serde_json::to_vec(&self.manifest).unwrap();
        let message = signing_message(Purpose::Software, &self.keys[0].id, &payload).unwrap();
        serde_json::to_vec(&SignedEnvelope {
            format: "rela.signed.v1".into(),
            key_id: self.keys[0].id.clone(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(self.signing.sign(&message).to_bytes()),
        })
        .unwrap()
    }

    fn plan(&self) -> Result<InstallerPlan, AppError> {
        InstallerPlan::from_signed(
            &self.envelope(),
            &self.keys,
            Channel::Stable,
            &Version::parse("0.1.0").unwrap(),
            &self.manifest.version,
        )
    }

    fn app(&self) -> tauri::App<MockRuntime> {
        self.app_configured(|_| {})
    }

    fn app_configured(
        &self,
        change: impl FnOnce(&mut serde_json::Value),
    ) -> tauri::App<MockRuntime> {
        let mut context = mock_context(noop_assets());
        let mut config = serde_json::json!({
            "pubkey": self.public_key,
            "endpoints": [],
            "requireSignedVersion": true,
            "dangerousInsecureTransportProtocol": true,
            "windows": { "installMode": "passive" }
        });
        change(&mut config);
        context
            .config_mut()
            .plugins
            .0
            .insert("updater".into(), config);
        mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap()
    }

    async fn official(&self) -> OfficialInstaller {
        let app = self.app();
        self.plan()
            .unwrap()
            .prepare(app.handle(), &self.directory.join("Rela.exe"))
            .await
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_file(self.directory.join("setup.exe")).unwrap();
        fs::remove_dir(&self.directory).unwrap();
    }
}

#[test]
fn official_check_reads_verified_local_manifest_and_keeps_package_locked() {
    tauri::async_runtime::block_on(async {
        let fixture = Fixture::new("timestamp:1\tversion:0.2.0");
        let official = fixture.official().await;
        let verified = official
            .verify(&fixture.directory.join("setup.exe"))
            .unwrap();
        assert!(fs::OpenOptions::new()
            .write(true)
            .open(fixture.directory.join("setup.exe"))
            .is_err());
        assert!(fs::remove_file(fixture.directory.join("setup.exe")).is_err());
        // An unbound adapter is verification-only: launch refuses before any
        // installer code runs, even for a correctly signed package.
        assert_eq!(
            verified.launch().unwrap_err().code,
            "installer_update_invalid"
        );
        assert_eq!(
            fs::read(fixture.directory.join("setup.exe")).unwrap(),
            fixture.artifact
        );
    });
}

#[test]
fn transaction_binding_requires_exact_feed_version_channel_and_non_recovery_request() {
    use rela_lib::updates::{
        ipc::{self, Listener, Role},
        nsis_worker::{self, Request},
        process::ProcessTicket,
    };
    tauri::async_runtime::block_on(async {
        let fixture = Fixture::new("timestamp:1\tversion:0.2.0");
        let id = ipc::random_id().unwrap();
        let control = fixture.directory.join(format!(".rela-installed-user-{id}"));
        fs::create_dir(&control).unwrap();
        let mut request = Request {
            schema_version: 1,
            id: id.clone(),
            root: fixture.directory.canonicalize().unwrap(),
            previous_version: Version::new(0, 1, 0),
            channel: Channel::Stable,
            envelope_digest: sha256(&fixture.envelope()),
            runner_digest: "a".repeat(64),
            controller: ProcessTicket {
                pid: 1,
                started_at: "1".into(),
            },
            pre: Listener::bind(&id, Role::InstallerPre).unwrap().endpoint(),
            post: Listener::bind(&id, Role::InstallerPost).unwrap().endpoint(),
            recovery: None,
        };
        let app = fixture.app();
        nsis_worker::write_request(&control, &request).unwrap();
        let official = fixture
            .plan()
            .unwrap()
            .prepare_transaction(app.handle(), &control.join(nsis_worker::REQUEST_FILE))
            .await
            .unwrap();
        drop(
            official
                .verify(&fixture.directory.join("setup.exe"))
                .unwrap(),
        );
        for field in ["digest", "version", "channel", "recovery"] {
            match field {
                "digest" => request.envelope_digest = "b".repeat(64),
                "version" => request.previous_version = Version::new(0, 0, 1),
                "channel" => request.channel = Channel::Test,
                _ => {
                    request.recovery = Some(
                        Listener::bind(&id, Role::InstallerRecovery)
                            .unwrap()
                            .endpoint(),
                    )
                }
            }
            nsis_worker::write_request(&control, &request).unwrap();
            assert!(
                fixture
                    .plan()
                    .unwrap()
                    .prepare_transaction(app.handle(), &control.join(nsis_worker::REQUEST_FILE))
                    .await
                    .is_err(),
                "accepted {field}"
            );
            request.envelope_digest = sha256(&fixture.envelope());
            request.previous_version = Version::new(0, 1, 0);
            request.channel = Channel::Stable;
            request.recovery = None;
        }
        fs::remove_file(control.join(nsis_worker::REQUEST_FILE)).unwrap();
        fs::remove_dir(control).unwrap();
    });
}

#[test]
fn invalid_selection_rejected_before_network_or_files() {
    let mut fixture = Fixture::new("version:0.2.0");
    let current = Version::parse("0.1.0").unwrap();
    assert!(InstallerPlan::from_signed(
        &fixture.envelope(),
        &fixture.keys,
        Channel::Test,
        &current,
        &fixture.manifest.version
    )
    .is_err());
    assert!(InstallerPlan::from_signed(
        &fixture.envelope(),
        &fixture.keys,
        Channel::Stable,
        &current,
        &Version::parse("0.3.0").unwrap()
    )
    .is_err());
    assert!(InstallerPlan::from_signed(
        &fixture.envelope(),
        &fixture.keys,
        Channel::Stable,
        &fixture.manifest.version,
        &fixture.manifest.version
    )
    .is_err());
    let mut envelope: serde_json::Value = serde_json::from_slice(&fixture.envelope()).unwrap();
    envelope["payload"] = STANDARD.encode(b"{}").into();
    assert!(InstallerPlan::from_signed(
        &serde_json::to_vec(&envelope).unwrap(),
        &fixture.keys,
        Channel::Stable,
        &current,
        &fixture.manifest.version
    )
    .is_err());
    fixture.manifest.minimum_app_version = Version::parse("0.1.1").unwrap();
    assert!(fixture.plan().is_err());
    fixture.manifest.minimum_app_version = current;
    fixture.manifest.installer.size = 512 * 1024 * 1024 + 1;
    assert!(fixture.plan().is_err());
    fixture.manifest.installer.size = fixture.artifact.len() as u64;
    fixture.manifest.published_at = "tomorrow".into();
    assert!(fixture.plan().is_err());
}

#[test]
fn artifact_tamper_and_missing_or_wrong_signed_version_never_become_launchable() {
    tauri::async_runtime::block_on(async {
        for comment in [
            "timestamp:1",
            "version:0.1.0",
            "version:0.2.0\tversion:0.1.0",
        ] {
            let fixture = Fixture::new(comment);
            assert!(fixture
                .official()
                .await
                .verify(&fixture.directory.join("setup.exe"))
                .is_err());
            assert_eq!(
                fs::read(fixture.directory.join("setup.exe")).unwrap(),
                fixture.artifact
            );
        }
        let fixture = Fixture::new("version:0.2.0");
        let official = fixture.official().await;
        fs::write(fixture.directory.join("setup.exe"), b"MZ tampered artifact").unwrap();
        assert!(official
            .verify(&fixture.directory.join("setup.exe"))
            .is_err());
        assert_eq!(
            fs::read(fixture.directory.join("setup.exe")).unwrap(),
            b"MZ tampered artifact"
        );

        let mut fixture = Fixture::new("timestamp:1\tversion:0.2.0");
        let comment_tamper = String::from_utf8(
            STANDARD
                .decode(&fixture.manifest.installer.signature)
                .unwrap(),
        )
        .unwrap()
        .replace("timestamp:1", "timestamp:2");
        fixture.manifest.installer.signature = STANDARD.encode(comment_tamper);
        assert!(fixture
            .official()
            .await
            .verify(&fixture.directory.join("setup.exe"))
            .is_err());
    });
}

#[test]
fn unsafe_updater_configuration_is_rejected_before_any_request() {
    tauri::async_runtime::block_on(async {
        let fixture = Fixture::new("version:0.2.0");
        for change in 0..6 {
            let app = fixture.app_configured(|config| match change {
                0 => config["endpoints"] = serde_json::json!(["https://example.com/unsigned.json"]),
                1 => config["allowDowngrades"] = true.into(),
                2 => config["requireSignedVersion"] = false.into(),
                3 => config["dangerousAcceptInvalidCerts"] = true.into(),
                4 => config["dangerousAcceptInvalidHostnames"] = true.into(),
                _ => config["windows"]["installerArgs"] = serde_json::json!(["/D=C:/unexpected"]),
            });
            assert!(
                fixture
                    .plan()
                    .unwrap()
                    .prepare(app.handle(), &fixture.directory.join("Rela.exe"))
                    .await
                    .is_err(),
                "config change {change}"
            );
        }
    });
}
