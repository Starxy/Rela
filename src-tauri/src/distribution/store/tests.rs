use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use rela_manifests::{signing_message, SignedEnvelope};

struct Fixture {
    root: PathBuf,
    key: SigningKey,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rela-config-test-{}", unique_id()));
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            key: SigningKey::from_bytes(&[73; 32]),
        }
    }
    fn store(&self) -> ConfigStore {
        ConfigStore::new(
            self.root.clone(),
            vec![PublicKey {
                id: "test-only".into(),
                purpose: Purpose::Resources,
                public_key: STANDARD.encode(self.key.verifying_key().to_bytes()),
            }],
        )
    }
    fn envelope(&self, version: u64, network: &str) -> Vec<u8> {
        let mut value: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../../config/resources.example.json"))
                .unwrap();
        value["version"] = version.into();
        value["network"] = network.into();
        let payload = serde_json::to_vec(&value).unwrap();
        let signature = self
            .key
            .sign(&signing_message(Purpose::Resources, "test-only", &payload).unwrap());
        serde_json::to_vec(&SignedEnvelope {
            format: "rela.signed.v1".into(),
            key_id: "test-only".into(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signature.to_bytes()),
        })
        .unwrap()
    }
    fn apply(&self, version: u64) -> SyncStatus {
        let store = self.store();
        store
            .apply_resources(
                store.begin_refresh(true).unwrap().unwrap(),
                &self.envelope(version, "lab201"),
                Some(format!("\"v{version}\"")),
            )
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Fixed temp test directory, created uniquely by this fixture.
        let expected = std::env::temp_dir();
        if self.root.parent() == Some(expected.as_path())
            && self
                .root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("rela-config-test-")
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn update(view: NetworkConfigView, secret: Option<String>) -> NetworkConfigUpdate {
    NetworkConfigUpdate {
        network_name: view.network_name,
        peers: view.peers,
        credential_secret: secret,
        private_mode: view.private_mode,
        disable_p2p: view.disable_p2p,
        gateway_ip: view.gateway_ip,
    }
}

#[test]
fn local_override_persists_and_manual_cached_refresh_restores_defaults_only() {
    let fixture = Fixture::new();
    assert!(!fixture.store().status().unwrap().configuration_ready);
    fixture.apply(1);
    let store = fixture.store();
    let mut view = store.configuration().unwrap();
    view.peers = vec!["tcp://127.0.0.1:2222".into()];
    view.private_mode = false;
    view.disable_p2p = false;
    view.gateway_ip = Some("192.168.200.254".into());
    store
        .save(update(view, Some(STANDARD.encode([21; 32]))))
        .unwrap();
    let restarted = fixture.store();
    assert!(restarted.status().unwrap().local_override);
    assert!(restarted.begin_refresh(false).unwrap().is_none());
    assert_eq!(restarted.resources().unwrap().len(), 1);
    let mut restored = restarted.configuration().unwrap();
    restored.peers = vec!["tcp://47.93.55.228:12010".into()];
    restarted.save(update(restored, None)).unwrap();
    assert!(fixture.store().begin_refresh(false).unwrap().is_none());
    let ticket = restarted.begin_refresh(true).unwrap().unwrap();
    assert_eq!(ticket.etag.as_deref(), Some("\"v1\""));
    let bytes = ticket.cached_envelope.clone().unwrap();
    let status = restarted
        .apply_resources(ticket, &bytes, Some("\"v1\"".into()))
        .unwrap();
    assert!(!status.local_override);
    let view = restarted.configuration().unwrap();
    assert_eq!(view.peers, vec!["tcp://47.93.55.228:12010"]);
    assert!(view.has_credential);
    assert!(!view.private_mode && !view.disable_p2p);
    assert_eq!(view.gateway_ip.as_deref(), Some("192.168.200.254"));
}

#[test]
fn in_flight_auto_and_manual_requests_cannot_overwrite_newer_edits() {
    let fixture = Fixture::new();
    fixture.apply(1);
    for manual in [false, true] {
        let store = fixture.store();
        let ticket = store.begin_refresh(manual).unwrap().unwrap();
        let mut view = store.configuration().unwrap();
        view.private_mode = !view.private_mode;
        store.save(update(view, None)).unwrap();
        let error = store
            .apply_resources(ticket, &fixture.envelope(2, "lab201"), None)
            .unwrap_err();
        assert_eq!(error.code, "configuration_changed");
        assert_eq!(store.status().unwrap().resource_version, Some(1));
    }
    let store = fixture.store();
    let ticket = store.begin_refresh(false).unwrap().unwrap();
    let mut view = store.configuration().unwrap();
    view.peers = vec!["tcp://backup.local:2222".into()];
    store.save(update(view, None)).unwrap();
    assert!(store
        .apply_resources(ticket, &fixture.envelope(2, "lab201"), None)
        .is_err());
    assert_eq!(
        fixture.store().configuration().unwrap().peers,
        vec!["tcp://backup.local:2222"]
    );
}

#[test]
fn tamper_replay_and_same_revision_changes_keep_previous_settings() {
    let fixture = Fixture::new();
    fixture.apply(2);
    let store = fixture.store();
    let original = fs::read(fixture.root.join("configuration.json")).unwrap();
    for bytes in [
        fixture.envelope(1, "lab201"),
        fixture.envelope(2, "other"),
        b"unsigned".to_vec(),
    ] {
        let ticket = store.begin_refresh(true).unwrap().unwrap();
        assert!(store.apply_resources(ticket, &bytes, None).is_err());
        assert_eq!(
            fs::read(fixture.root.join("configuration.json")).unwrap(),
            original
        );
    }
    let state = store.read_state().unwrap();
    let cache = state.resources.unwrap();
    fs::write(
        fixture
            .root
            .join("cache")
            .join(format!("resources-{}.json", cache.envelope_digest)),
        b"tampered",
    )
    .unwrap();
    assert!(store.resources().is_err());
    let ticket = store.begin_refresh(true).unwrap().unwrap();
    assert!(ticket.etag.is_none() && ticket.cached_envelope.is_none());
    assert!(store
        .apply_resources(ticket, &fixture.envelope(1, "lab201"), None)
        .is_err());
    fixture.apply(3);
    assert_eq!(store.resources().unwrap().len(), 1);
}

#[test]
fn credentials_are_separate_and_reimport_recovers_unreadable_dpapi() {
    let fixture = Fixture::new();
    fixture.apply(1);
    let store = fixture.store();
    let secret = STANDARD.encode([19; 32]);
    store
        .save(update(store.configuration().unwrap(), Some(secret.clone())))
        .unwrap();
    let bytes = fs::read(fixture.root.join("configuration.json")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains(&secret));
    assert!(!String::from_utf8_lossy(&bytes).contains("credential_secret"));
    let state = store.read_state().unwrap();
    let path = fixture
        .root
        .join("credentials")
        .join(format!("{}.dat", state.credential.unwrap().id));
    let encrypted = fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&encrypted).contains(&secret));
    assert!(store.connection_snapshot().is_ok());
    fs::write(path, b"unreadable on this user").unwrap();
    assert!(!store.configuration().unwrap().has_credential);
    assert!(store.status().unwrap().credential_unavailable);
    assert!(store.connection_snapshot().is_err());
    store
        .save(update(store.configuration().unwrap(), Some(secret)))
        .unwrap();
    assert!(!store.status().unwrap().credential_unavailable);
    assert!(store.connection_snapshot().is_ok());
}

#[test]
fn public_manual_settings_can_connect_without_a_resource_cache() {
    let fixture = Fixture::new();
    let store = fixture.store();
    assert!(store.connection_snapshot().is_err());
    let mut view = store.configuration().unwrap();
    view.network_name = "lab201".into();
    view.peers = vec!["tcp://127.0.0.1:10000".into()];
    store
        .save(update(view, Some(STANDARD.encode([20; 32]))))
        .unwrap();
    assert!(store.connection_snapshot().is_ok());
    assert!(store.resources().unwrap().is_empty());
    assert!(store.begin_refresh(false).unwrap().is_none());
    let snapshot = store.connection_snapshot().unwrap();
    store.mark_applied(snapshot).unwrap();
    assert!(!store.status().unwrap().pending_reconnect);
    let mut view = store.configuration().unwrap();
    view.peers = vec!["tcp://127.0.0.1:10001".into()];
    store.save(update(view, None)).unwrap();
    assert!(store.status().unwrap().pending_reconnect);
}

#[test]
fn failed_pointer_replace_does_not_commit_staged_public_or_private_objects() {
    let fixture = Fixture::new();
    fixture.apply(1);
    let store = fixture.store();
    let path = fixture.root.join("configuration.json");
    let original = fs::read(&path).unwrap();
    let original_permissions = fs::metadata(&path).unwrap().permissions();
    let mut readonly = original_permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&path, readonly).unwrap();
    let ticket = store.begin_refresh(true).unwrap().unwrap();
    let result = store.apply_resources(ticket, &fixture.envelope(2, "lab201"), None);
    fs::set_permissions(&path, original_permissions).unwrap();
    assert!(result.is_err());
    assert_eq!(fs::read(path).unwrap(), original);
    assert_eq!(fixture.store().status().unwrap().resource_version, Some(1));
}

#[test]
fn old_password_is_never_reused_and_original_encrypted_file_is_preserved() {
    let fixture = Fixture::new();
    let legacy = serde_json::json!({ "network_name": "lab201", "network_secret": "synthetic-old-password",
        "peers": ["tcp://127.0.0.1:1111"], "private_mode": false, "disable_p2p": true, "gateway_ip": null });
    let path = fixture.root.join("network.dat");
    let encrypted = platform::protect(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    fs::write(&path, &encrypted).unwrap();
    let store = fixture.store();
    assert!(!store.configuration().unwrap().has_credential);
    assert!(store.status().unwrap().local_override);
    assert!(store.connection_snapshot().is_err());
    assert_eq!(fs::read(path).unwrap(), encrypted);
    assert!(
        !String::from_utf8_lossy(&fs::read(fixture.root.join("configuration.json")).unwrap())
            .contains("synthetic-old-password")
    );
}

#[test]
fn network_binding_and_cross_process_lock_are_preserved() {
    let fixture = Fixture::new();
    fixture.apply(1);
    let store = fixture.store();
    store
        .save(update(
            store.configuration().unwrap(),
            Some(STANDARD.encode([25; 32])),
        ))
        .unwrap();
    let mut changed = store.configuration().unwrap();
    changed.network_name = "different".into();
    assert!(store.save(update(changed, None)).is_err());
    let ticket = store.begin_refresh(true).unwrap().unwrap();
    store
        .apply_resources(ticket, &fixture.envelope(2, "different"), None)
        .unwrap();
    assert!(!store.configuration().unwrap().has_credential);
    let lock = store.lock().unwrap();
    assert!(fixture.store().configuration().is_err());
    drop(lock);
    assert!(fixture.store().configuration().is_ok());
}

#[test]
fn simultaneous_ui_reads_share_the_same_process_transaction() {
    let fixture = Fixture::new();
    fixture.apply(1);
    let store = std::sync::Arc::new(fixture.store());
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let store = std::sync::Arc::clone(&store);
            std::thread::spawn(move || {
                for _ in 0..20 {
                    assert_eq!(store.resources().unwrap().len(), 1);
                    assert!(store.status().unwrap().configuration_ready);
                    assert_eq!(store.configuration().unwrap().network_name, "lab201");
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}
