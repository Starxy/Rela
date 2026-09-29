use super::*;
use crate::distribution::package::tests::signed;
use std::{
    io::Cursor,
    time::{SystemTime, UNIX_EPOCH},
};
use zip::{write::SimpleFileOptions, ZipWriter};

fn pe() -> Vec<u8> {
    // Header-only test bytes, never executed.
    let mut bytes = vec![0u8; 256];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
    bytes[64..68].copy_from_slice(b"PE\0\0");
    bytes[68..70].copy_from_slice(&0x8664u16.to_le_bytes());
    bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes
}
pub(crate) fn fixture_zip(case: &str) -> Vec<u8> {
    let mut payloads: BTreeMap<String, Vec<u8>> = UPDATE_FILES
        .iter()
        .map(|name| {
            (
                name.to_string(),
                if name.ends_with(".exe") {
                    pe()
                } else {
                    b"test license or helper".to_vec()
                },
            )
        })
        .collect();
    if case == "x86" {
        payloads.get_mut("Rela.exe").unwrap()[68..70].copy_from_slice(&0x14cu16.to_le_bytes());
    }
    let engines: BTreeMap<String, String> = [
        "easytier-core.exe",
        "easytier-cli.exe",
        "wintun.dll",
        "Packet.dll",
        "WinDivert64.sys",
    ]
    .iter()
    .map(|name| {
        (
            name.to_string(),
            format!(
                "{:x}",
                Sha256::digest(&payloads[&format!("easytier/{name}")])
            ),
        )
    })
    .collect();
    payloads.insert(
        "easytier/manifest.json".into(),
        serde_json::to_vec(
            &serde_json::json!({ "version": "2.7.0-test", "engine_revision": 2, "files": engines }),
        )
        .unwrap(),
    );
    let mut index = PortableIndex {
        schema_version: 1,
        version: Version::new(0, 2, 0),
        target: "windows-x86_64".into(),
        profile: "release".into(),
        core_version: "2.7.0-test".into(),
        engine_revision: 2,
        files: payloads
            .iter()
            .map(|(name, bytes)| (name.clone(), format!("{:x}", Sha256::digest(bytes))))
            .collect(),
    };
    match case {
        "wrong-hash" => {
            index.files.insert("README.txt".into(), "0".repeat(64));
        }
        "version" => index.version = Version::new(0, 1, 0),
        "debug" => index.profile = "debug".into(),
        "target" => index.target = "windows-aarch64".into(),
        "core" => index.engine_revision += 1,
        "missing" => {
            payloads.remove("README.txt");
        }
        _ => {}
    }
    payloads.insert("checksums.json".into(), serde_json::to_vec(&index).unwrap());
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let root = "Rela_0.2.0_x64-update";
    for (name, bytes) in payloads {
        zip.start_file(format!("{root}/{name}"), options).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    match case {
        "valid" | "wrong-hash" | "version" | "debug" | "target" | "core" | "x86" | "missing" => {}
        "symlink" => {
            zip.add_symlink(format!("{root}/link"), "../outside", options)
                .unwrap();
        }
        invalid => {
            zip.start_file(format!("{root}/{invalid}"), options)
                .unwrap();
            zip.write_all(b"unexpected").unwrap();
        }
    }
    zip.finish().unwrap().into_inner()
}

struct Fixture {
    base: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "rela-portable-stage-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&base).unwrap();
        fs::write(base.join("Rela.exe"), b"old program").unwrap();
        fs::create_dir(base.join("data")).unwrap();
        fs::write(
            base.join("data/credential.dat"),
            b"synthetic encrypted user data",
        )
        .unwrap();
        fs::write(base.join("portable.txt"), b"existing marker").unwrap();
        Self { base }
    }
    fn run(&self, case: &str) -> Result<PortableStage, AppError> {
        let bytes = fixture_zip(case);
        let (package, key) = signed(&bytes);
        let manifest = SoftwareManifest {
            schema_version: 1,
            revision: 1,
            version: Version::new(0, 2, 0),
            channel: rela_manifests::Channel::Stable,
            target: "windows-x86_64".into(),
            notes: "test".into(),
            published_at: "2026-09-29T00:00:00Z".into(),
            core_version: "2.7.0-test".into(),
            minimum_app_version: Version::new(0, 1, 0),
            installer: rela_manifests::Package {
                url: package.url.replace(".zip", ".exe"),
                ..package.clone()
            },
            portable: package.clone(),
        };
        let path = self.base.join("update.zip");
        fs::write(&path, bytes).unwrap();
        let mut verified = VerifiedPackage::open(&path, &package, &key).unwrap();
        stage(&mut verified, &manifest, &self.base.join("stage"))
    }
    fn assert_user_files(&self) {
        assert_eq!(
            fs::read(self.base.join("Rela.exe")).unwrap(),
            b"old program"
        );
        assert_eq!(
            fs::read(self.base.join("data/credential.dat")).unwrap(),
            b"synthetic encrypted user data"
        );
        assert_eq!(
            fs::read(self.base.join("portable.txt")).unwrap(),
            b"existing marker"
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let path = self.base.canonicalize().unwrap();
        let parent = std::env::temp_dir().canonicalize().unwrap();
        assert_eq!(path.parent(), Some(parent.as_path()));
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("rela-portable-stage-"));
        fs::remove_dir_all(path).unwrap();
    }
}

#[test]
fn signed_package_stages_only_complete_program_files_and_preserves_user_data() {
    let fixture = Fixture::new();
    let staged = fixture.run("valid").unwrap();
    assert_eq!(staged.index.version, Version::new(0, 2, 0));
    for name in UPDATE_FILES {
        assert!(staged.directory.join(name).is_file());
    }
    assert!(!staged.directory.join("data").exists());
    assert!(!staged.directory.join("portable.txt").exists());
    fixture.assert_user_files();
}

#[test]
fn unsafe_archive_names_links_duplicates_and_data_are_rejected() {
    for case in [
        "../escape",
        "C:/escape",
        "data/config.json",
        "portable.txt",
        "Rela.exe:stream",
        "rela.exe",
        "symlink",
        "easytier/../../escape",
        "easytier\\escape",
        "CON",
        "Rela.exe.",
    ] {
        let fixture = Fixture::new();
        assert!(fixture.run(case).is_err(), "accepted {case}");
        assert!(!fixture.base.join("stage").exists());
        fixture.assert_user_files();
    }
}

#[test]
fn wrong_version_architecture_hash_core_or_incomplete_package_never_becomes_ready() {
    for case in [
        "wrong-hash",
        "version",
        "debug",
        "target",
        "core",
        "x86",
        "missing",
    ] {
        let fixture = Fixture::new();
        assert!(fixture.run(case).is_err(), "accepted {case}");
        assert!(!fixture.base.join("stage").exists());
        fixture.assert_user_files();
    }
}

#[test]
fn existing_destination_is_never_overwritten() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.base.join("stage")).unwrap();
    fs::write(fixture.base.join("stage/keep"), b"existing").unwrap();
    assert!(fixture.run("valid").is_err());
    assert_eq!(
        fs::read(fixture.base.join("stage/keep")).unwrap(),
        b"existing"
    );
    fixture.assert_user_files();
}

#[cfg(windows)]
#[test]
fn signed_program_guard_prevents_writes_and_deletes_until_the_helper_finishes() {
    let fixture = Fixture::new();
    let staged = fixture.run("valid").unwrap();
    let guard = staged.lock().unwrap();
    for name in ["Rela.exe", "easytier/easytier-core.exe", "checksums.json"] {
        assert!(fs::write(staged.directory.join(name), b"changed").is_err());
        assert!(fs::remove_file(staged.directory.join(name)).is_err());
    }
    staged.verify().unwrap();
    drop(guard);
    fs::write(staged.directory.join("Rela.exe"), b"changed").unwrap();
    assert!(staged.lock().is_err());
    fixture.assert_user_files();
}
