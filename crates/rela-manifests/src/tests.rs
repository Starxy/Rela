use super::*;
use ed25519_dalek::{Signer, SigningKey};

const SAMPLE: &[u8] = include_bytes!("../../../config/resources.example.json");

fn signed(payload: &[u8], purpose: Purpose) -> (Vec<u8>, Vec<PublicKey>) {
    let key = SigningKey::from_bytes(&[42; 32]);
    let id = "synthetic-test-key";
    let signature = key.sign(&signing_message(purpose, id, payload).unwrap());
    let envelope = SignedEnvelope {
        format: "rela.signed.v1".into(),
        key_id: id.into(),
        payload: STANDARD.encode(payload),
        signature: STANDARD.encode(signature.to_bytes()),
    };
    (
        serde_json::to_vec(&envelope).unwrap(),
        vec![PublicKey {
            id: id.into(),
            purpose,
            public_key: STANDARD.encode(key.verifying_key().to_bytes()),
        }],
    )
}

#[test]
fn signed_sample_requires_matching_key_purpose_and_unmodified_bytes() {
    let (signed, keys) = signed(SAMPLE, Purpose::Resources);
    let payload = verify_envelope(&signed, Purpose::Resources, &keys).unwrap();
    let manifest = ResourceManifest::parse(&payload).unwrap();
    assert_eq!(
        manifest.resources[0].probe_target().unwrap(),
        ("192.168.200.10".into(), 22)
    );
    assert!(verify_envelope(&signed, Purpose::Software, &keys).is_err());
    assert!(verify_envelope(&signed, Purpose::Resources, &[]).is_err());
    let mut tampered: SignedEnvelope = serde_json::from_slice(&signed).unwrap();
    tampered.payload = STANDARD.encode(SAMPLE.iter().copied().chain(*b" ").collect::<Vec<_>>());
    assert!(verify_envelope(
        &serde_json::to_vec(&tampered).unwrap(),
        Purpose::Resources,
        &keys
    )
    .is_err());
    let mut duplicated = keys.clone();
    duplicated.extend(keys);
    assert!(verify_envelope(&signed, Purpose::Resources, &duplicated).is_err());
    assert!(verify_envelope(&vec![b' '; MAX_ENVELOPE_BYTES + 1], Purpose::Resources, &[]).is_err());
}

#[test]
fn node_generated_fixture_verifies_in_rust() {
    let keys: Vec<PublicKey> =
        serde_json::from_slice(include_bytes!("../tests/fixtures/keys.json")).unwrap();
    let payload = verify_envelope(
        include_bytes!("../tests/fixtures/resources.signed.json"),
        Purpose::Resources,
        &keys,
    )
    .unwrap();
    assert_eq!(ResourceManifest::parse(&payload).unwrap().network, "lab201");
}

#[test]
fn rejects_credentials_device_preferences_unknown_schema_and_duplicate_resources() {
    for field in [
        "credential_secret",
        "network_secret",
        "private_mode",
        "gateway_ip",
        "disable_p2p",
        "device_name",
    ] {
        let mut value: serde_json::Value = serde_json::from_slice(SAMPLE).unwrap();
        value[field] = true.into();
        assert!(
            ResourceManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
            "accepted {field}"
        );
    }
    let mut value: serde_json::Value = serde_json::from_slice(SAMPLE).unwrap();
    value["resources"][0]["availability"] = "reachable".into();
    assert!(ResourceManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut manifest = ResourceManifest::parse(SAMPLE).unwrap();
    manifest.resources.push(manifest.resources[0].clone());
    assert!(manifest.validate().is_err());
    manifest.resources.pop();
    manifest.schema_version = 2;
    assert!(manifest.validate().is_err());
}

#[test]
fn revision_is_monotonic_and_content_cannot_change_at_same_revision() {
    assert!(check_revision(0, "x", None).is_err());
    assert!(check_revision(MAX_REVISION + 1, "x", None).is_err());
    assert!(check_revision(1, "x", Some((2, "x"))).is_err());
    assert!(check_revision(2, "y", Some((2, "x"))).is_err());
    assert!(check_revision(2, "x", Some((2, "x"))).is_ok());
    assert!(check_revision(3, "x", Some((2, "y"))).is_ok());
}

#[test]
fn validates_resource_protocols_without_shell_or_credential_inputs() {
    let mut resource = ResourceManifest::parse(SAMPLE).unwrap().resources.remove(0);
    for host in [
        "-oProxyCommand=cmd",
        "host;calc",
        "user@host",
        "host\nother",
        "tcp://host:22",
        "host/path",
    ] {
        resource.address = host.into();
        assert!(resource.validate().is_err());
    }
    resource.address = "::1".into();
    assert!(resource.validate().is_ok());
    resource.username = Some("-oProxyCommand".into());
    assert!(resource.validate().is_err());
    resource.username = None;
    resource.kind = ResourceKind::Web;
    resource.port = None;
    for address in [
        "file:///C:/Windows",
        "javascript:alert(1)",
        "https://user:secret@example.com/",
        "http://host:0/",
    ] {
        resource.address = address.into();
        assert!(resource.validate().is_err());
    }
    resource.address = "https://[::1]/dashboard?q=a".into();
    assert_eq!(resource.probe_target().unwrap(), ("::1".into(), 443));
    resource.kind = ResourceKind::Nas;
    for address in [
        r"C:\Windows",
        r"\\?\C:\Windows",
        r"\\host\share\..\other",
        r"\\host\share:ads",
        r"\\host\share\.",
        r"\\host\share\bad.",
    ] {
        resource.address = address.into();
        assert!(resource.validate().is_err(), "accepted unsafe NAS path");
    }
    resource.address = r"\\nas.lab\共享目录\项目 A".into();
    assert_eq!(resource.probe_target().unwrap(), ("nas.lab".into(), 445));
}

fn software() -> SoftwareManifest {
    SoftwareManifest {
        schema_version: 2,
        revision: 1,
        version: Version::new(0, 2, 0),
        channel: Channel::Stable,
        target: "windows-x86_64".into(),
        notes: "测试更新".into(),
        published_at: "2026-09-29T00:00:00Z".into(),
    }
}

#[test]
fn software_is_channel_architecture_and_release_tag_bound() {
    let mut manifest = software();
    manifest.validate(Channel::Stable).unwrap();
    assert_eq!(
        manifest.release_url(),
        format!("{REPOSITORY}/releases/tag/v0.2.0")
    );
    assert!(manifest.validate(Channel::Test).is_err());
    manifest.target = "windows-aarch64".into();
    assert!(manifest.validate(Channel::Stable).is_err());
    manifest = software();
    manifest.version = Version::parse("0.3.0-beta.1").unwrap();
    assert!(manifest.validate(Channel::Stable).is_err());
    manifest.channel = Channel::Test;
    manifest.validate(Channel::Test).unwrap();
    assert_eq!(
        manifest.release_url(),
        format!("{REPOSITORY}/releases/tag/v0.3.0-beta.1")
    );
    manifest.version = Version::parse("0.3.0+local").unwrap();
    assert!(manifest.validate(Channel::Test).is_err());
}

#[test]
fn discovery_manifest_rejects_installation_fields_and_arbitrary_urls() {
    for field in [
        "installer",
        "portable",
        "minimum_app_version",
        "release_url",
    ] {
        let mut value = serde_json::to_value(software()).unwrap();
        value[field] = "https://example.com".into();
        assert!(
            SoftwareManifest::parse(&serde_json::to_vec(&value).unwrap(), Channel::Stable).is_err()
        );
    }
    let mut value = serde_json::to_value(software()).unwrap();
    value["version"] = "../../other".into();
    assert!(
        SoftwareManifest::parse(&serde_json::to_vec(&value).unwrap(), Channel::Stable).is_err()
    );
    value = serde_json::to_value(software()).unwrap();
    value["schema_version"] = 1.into();
    assert!(
        SoftwareManifest::parse(&serde_json::to_vec(&value).unwrap(), Channel::Stable).is_err()
    );
}
