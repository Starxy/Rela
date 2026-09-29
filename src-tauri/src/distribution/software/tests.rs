use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use rela_manifests::{signing_message, PublicKey, SignedEnvelope};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture {
    root: PathBuf,
    key: SigningKey,
    manager: SoftwareManager,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rela-software-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let key = SigningKey::from_bytes(&[29; 32]);
        let mut config = DistributionConfig::bundled().unwrap();
        config.keys = vec![PublicKey {
            id: "software-test-only".into(),
            purpose: Purpose::Software,
            public_key: STANDARD.encode(key.verifying_key().to_bytes()),
        }];
        let manager = SoftwareManager::new(root.clone(), config, InstallKind::Portable).unwrap();
        Self { root, key, manager }
    }
    fn signed(&self, revision: u64, version: &str, channel: &str, minimum: &str) -> Vec<u8> {
        let package = |extension| {
            serde_json::json!({
                "url": format!("https://github.com/Starxy/Rela/releases/download/v{version}/Rela.{extension}"),
                "size": if extension == "zip" { 234 } else { 123 }, "sha256": "b".repeat(64),
                "signature": STANDARD.encode(b"fixture signature")
            })
        };
        let payload = serde_json::to_vec(&serde_json::json!({
            "schema_version": 1, "revision": revision, "version": version, "channel": channel,
            "target": "windows-x86_64", "notes": "test", "published_at": "2026-09-29T00:00:00Z",
            "core_version": "isolated-core", "minimum_app_version": minimum,
            "installer": package("exe"), "portable": package("zip")
        }))
        .unwrap();
        let signature = self
            .key
            .sign(&signing_message(Purpose::Software, "software-test-only", &payload).unwrap());
        serde_json::to_vec(&SignedEnvelope {
            format: "rela.signed.v1".into(),
            key_id: "software-test-only".into(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signature.to_bytes()),
        })
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.parent() == Some(std::env::temp_dir().as_path())
            && self
                .root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("rela-software-test-")
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn signed_http_checks_use_conditional_reads_and_keep_cache_after_network_failure() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    let mut f = Fixture::new();
    let bytes = f.signed(3, "0.2.0", "stable", "0.1.0");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    f.manager.config.software_stable_url =
        format!("http://{}/software", listener.local_addr().unwrap());
    f.manager.fetcher = Fetcher::local_for_test();
    let server = thread::spawn(move || {
        for index in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..count]).to_lowercase();
            if index > 0 {
                assert!(request.contains("if-none-match: \"software-v3\""));
            }
            match index {
                0 => {
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"software-v3\"\r\nConnection: close\r\n\r\n", bytes.len()).unwrap();
                    stream.write_all(&bytes).unwrap();
                }
                1 => stream
                    .write_all(b"HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n")
                    .unwrap(),
                _ => stream
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap(),
            }
        }
    });
    tauri::async_runtime::block_on(async {
        for _ in 0..2 {
            let status = f.manager.check().await.unwrap();
            assert_eq!(status.candidate.unwrap().version, "0.2.0");
            assert!(!status.cached);
        }
        assert!(f.manager.check().await.is_err());
        let retained = f.manager.status().unwrap();
        assert_eq!(retained.candidate.unwrap().version, "0.2.0");
        assert!(retained.cached && retained.last_error.is_some());
    });
    server.join().unwrap();
}

#[test]
fn software_cache_survives_restart_without_reading_resource_override() {
    let f = Fixture::new();
    assert!(f.manager.status().unwrap().candidate.is_none());
    let bytes = f.signed(3, "0.2.0", "stable", "0.1.0");
    f.manager
        .accept(UpdateChannel::Stable, &bytes, Some("v3".into()))
        .unwrap();
    // Software storage must not try to deserialize the separate resource state.
    fs::write(
        f.root.join("configuration.json"),
        "unreadable-resource-state",
    )
    .unwrap();
    let restarted = SoftwareManager::new(
        f.root.clone(),
        f.manager.config.clone(),
        InstallKind::Installer,
    )
    .unwrap();
    let status = restarted.status().unwrap();
    assert!(status.cached);
    let candidate = status.candidate.unwrap();
    assert_eq!(candidate.version, "0.2.0");
    // Installed updates also download the signed Portable companion to verify
    // every NSIS output file and run the matching new recovery helper.
    assert_eq!(candidate.size, 123 + 234);
    assert!(!candidate.requires_manual_upgrade);
    assert_eq!(f.manager.status().unwrap().candidate.unwrap().size, 234);
}

#[test]
fn rollback_tampering_channel_confusion_and_failed_commit_preserve_previous() {
    let f = Fixture::new();
    f.manager
        .accept(
            UpdateChannel::Stable,
            &f.signed(3, "0.2.0", "stable", "0.1.0"),
            None,
        )
        .unwrap();
    for bytes in [
        f.signed(2, "0.3.0", "stable", "0.1.0"),
        f.signed(3, "0.3.0", "stable", "0.1.0"),
        f.signed(4, "0.1.0", "stable", "0.1.0"),
        f.signed(4, "0.3.0-beta.1", "test", "0.1.0"),
    ] {
        assert!(f
            .manager
            .accept(UpdateChannel::Stable, &bytes, None)
            .is_err());
    }
    let mut tampered: serde_json::Value =
        serde_json::from_slice(&f.signed(4, "0.3.0", "stable", "0.1.0")).unwrap();
    tampered["signature"] = STANDARD.encode([0_u8; 64]).into();
    assert!(f
        .manager
        .accept(
            UpdateChannel::Stable,
            &serde_json::to_vec(&tampered).unwrap(),
            None
        )
        .is_err());
    assert_eq!(
        f.manager.status().unwrap().candidate.unwrap().version,
        "0.2.0"
    );
    // Directory occupying the atomic pointer prevents a commit without erasing a prior backup.
    let pointer = f.manager.pointer_path(UpdateChannel::Stable);
    fs::rename(&pointer, pointer.with_extension("backup")).unwrap();
    fs::create_dir(&pointer).unwrap();
    assert!(f
        .manager
        .accept(
            UpdateChannel::Stable,
            &f.signed(4, "0.3.0", "stable", "0.1.0"),
            None
        )
        .is_err());
    fs::remove_dir(&pointer).unwrap();
    fs::rename(pointer.with_extension("backup"), &pointer).unwrap();
    assert_eq!(
        f.manager.status().unwrap().candidate.unwrap().version,
        "0.2.0"
    );
}

#[test]
fn explicit_test_channel_keeps_independent_cache_and_compatibility_gate() {
    let f = Fixture::new();
    f.manager
        .accept(
            UpdateChannel::Stable,
            &f.signed(9, "0.2.0", "stable", "0.1.0"),
            None,
        )
        .unwrap();
    f.manager
        .accept(
            UpdateChannel::Test,
            &f.signed(1, "0.3.0-beta.1", "test", "0.2.0"),
            None,
        )
        .unwrap();
    let candidate = f
        .manager
        .set_channel(UpdateChannel::Test)
        .unwrap()
        .candidate
        .unwrap();
    assert_eq!(candidate.version, "0.3.0-beta.1");
    assert!(candidate.requires_manual_upgrade);
    let restarted = SoftwareManager::new(
        f.root.clone(),
        f.manager.config.clone(),
        InstallKind::Portable,
    )
    .unwrap();
    assert_eq!(restarted.status().unwrap().channel, UpdateChannel::Test);
    assert_eq!(
        restarted
            .set_channel(UpdateChannel::Stable)
            .unwrap()
            .candidate
            .unwrap()
            .version,
        "0.2.0"
    );
    let pointer = restarted.pointer(UpdateChannel::Stable).unwrap().unwrap();
    fs::write(
        f.root
            .join("cache")
            .join(format!("software-{}.json", pointer.envelope_digest)),
        "damaged",
    )
    .unwrap();
    assert!(restarted.status().is_err());
    assert!(restarted
        .accept(
            UpdateChannel::Stable,
            &f.signed(8, "0.3.0", "stable", "0.1.0"),
            None
        )
        .is_err());
    restarted
        .accept(
            UpdateChannel::Stable,
            &f.signed(9, "0.2.0", "stable", "0.1.0"),
            None,
        )
        .unwrap();
    assert!(restarted.status().unwrap().candidate.is_some());
}

#[test]
fn selecting_an_update_reverifies_the_exact_confirmed_version_and_upgrade_floor() {
    let f = Fixture::new();
    assert!(f.manager.select("0.2.0").is_err());
    f.manager
        .accept(
            UpdateChannel::Stable,
            &f.signed(1, "0.2.0", "stable", "0.1.0"),
            None,
        )
        .unwrap();
    let selected = f.manager.select("0.2.0").unwrap();
    assert_eq!(selected.manifest.version, Version::new(0, 2, 0));
    assert_eq!(selected.envelope, f.signed(1, "0.2.0", "stable", "0.1.0"));
    assert!(f.manager.select("0.1.0").is_err());
    f.manager
        .accept(
            UpdateChannel::Stable,
            &f.signed(2, "0.3.0", "stable", "0.2.0"),
            None,
        )
        .unwrap();
    assert!(f.manager.select("0.2.0").is_err());
    assert_eq!(
        f.manager.select("0.3.0").err().unwrap().code,
        "manual_upgrade_required"
    );
}

#[test]
fn same_version_cannot_replace_packages_core_or_upgrade_floor_at_a_new_revision() {
    let f = Fixture::new();
    f.manager
        .accept(
            UpdateChannel::Stable,
            &f.signed(1, "0.2.0", "stable", "0.1.0"),
            None,
        )
        .unwrap();
    for field in [
        "installer",
        "portable",
        "core_version",
        "minimum_app_version",
    ] {
        let envelope: SignedEnvelope =
            serde_json::from_slice(&f.signed(2, "0.2.0", "stable", "0.1.0")).unwrap();
        let mut payload: serde_json::Value =
            serde_json::from_slice(&STANDARD.decode(envelope.payload).unwrap()).unwrap();
        match field {
            "installer" | "portable" => payload[field]["sha256"] = "c".repeat(64).into(),
            "core_version" => payload[field] = "different-core".into(),
            _ => payload[field] = "0.1.1".into(),
        }
        let payload = serde_json::to_vec(&payload).unwrap();
        let signature = f
            .key
            .sign(&signing_message(Purpose::Software, "software-test-only", &payload).unwrap());
        let changed = serde_json::to_vec(&SignedEnvelope {
            format: "rela.signed.v1".into(),
            key_id: "software-test-only".into(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signature.to_bytes()),
        })
        .unwrap();
        assert!(
            f.manager
                .accept(UpdateChannel::Stable, &changed, None)
                .is_err(),
            "accepted {field}"
        );
        assert_eq!(f.manager.select("0.2.0").unwrap().manifest.revision, 1);
    }
    f.manager
        .accept(
            UpdateChannel::Stable,
            &f.signed(2, "0.2.0", "stable", "0.1.0"),
            None,
        )
        .unwrap();
    assert_eq!(f.manager.select("0.2.0").unwrap().manifest.revision, 2);
}
