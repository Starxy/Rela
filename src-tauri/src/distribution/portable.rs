//! Strict, non-executing preparation of a signed Portable update. User data and
//! the portable marker never enter the update payload or replacement file list.
use super::{
    manifest_error,
    package::{self, VerifiedPackage},
};
use rela_manifests::SoftwareManifest;
use rela_protocol::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use zip::{CompressionMethod, ZipArchive};

const MAX_EXPANDED: u64 = 2 * 1024 * 1024 * 1024;
const MAX_FILE: u64 = 512 * 1024 * 1024;
pub const UPDATE_FILES: &[&str] = &[
    "Rela.exe",
    "README.txt",
    "THIRD-PARTY-NOTICES.md",
    "Remove-Network-Service.cmd",
    "Remove-Network-Service.ps1",
    "easytier/easytier-core.exe",
    "easytier/easytier-cli.exe",
    "easytier/wintun.dll",
    "easytier/Packet.dll",
    "easytier/WinDivert64.sys",
    "easytier/manifest.json",
    "third-party-licenses/EasyTier-LGPL-3.0.txt",
    "third-party-licenses/GPL-3.0.txt",
    "third-party-licenses/WinDivert-LICENSE.txt",
    "third-party-licenses/Wintun-LICENSE.txt",
];

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PortableIndex {
    pub schema_version: u32,
    pub version: Version,
    pub target: String,
    pub profile: String,
    pub core_version: String,
    pub engine_revision: u64,
    pub files: BTreeMap<String, String>,
}
pub struct PortableStage {
    directory: PathBuf,
    index: PortableIndex,
}

/// Hold all authenticated program bytes against writes/deletion while a helper
/// or candidate may load them. Verification runs after every handle is acquired.
pub struct ProgramGuard {
    _files: Vec<File>,
}
pub(crate) fn lock_program(
    directory: &Path,
    index: &PortableIndex,
    strict: bool,
) -> Result<ProgramGuard, AppError> {
    let mut files = Vec::new();
    for name in UPDATE_FILES
        .iter()
        .copied()
        .chain(std::iter::once("checksums.json"))
    {
        let path = directory.join(name);
        package::plain_file(&path)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        files.push(options.open(path).map_err(|_| invalid())?);
    }
    if strict {
        verify_tree(directory, index)?;
    } else {
        verify_program_files(directory, index)?;
    }
    Ok(ProgramGuard { _files: files })
}

impl PortableStage {
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn index(&self) -> &PortableIndex {
        &self.index
    }

    /// A path or saved index alone is not trusted. Every consumer rechecks all bytes.
    pub fn verify(&self) -> Result<(), AppError> {
        verify_tree(&self.directory, &self.index)
    }
    pub fn lock(&self) -> Result<ProgramGuard, AppError> {
        lock_program(&self.directory, &self.index, true)
    }
}

pub(crate) fn verify_tree(directory: &Path, index: &PortableIndex) -> Result<(), AppError> {
    package::plain_directory(directory)?;
    let expected: BTreeSet<String> = UPDATE_FILES
        .iter()
        .map(|name| name.to_string())
        .chain(std::iter::once("checksums.json".into()))
        .collect();
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(directory).map_err(|_| invalid())? {
        let entry = entry.map_err(|_| invalid())?;
        let name = entry.file_name().into_string().map_err(|_| invalid())?;
        if matches!(name.as_str(), "easytier" | "third-party-licenses") {
            package::plain_directory(&entry.path())?;
            for child in fs::read_dir(entry.path()).map_err(|_| invalid())? {
                let child = child.map_err(|_| invalid())?;
                package::plain_file(&child.path())?;
                actual.insert(format!(
                    "{name}/{}",
                    child.file_name().into_string().map_err(|_| invalid())?
                ));
            }
        } else {
            package::plain_file(&entry.path())?;
            actual.insert(name);
        }
    }
    if actual != expected {
        return Err(invalid());
    }
    verify_program_files(directory, index)
}

/// Validate the program subset of an installed directory, preserving user files.
pub(crate) fn verify_program_files(
    directory: &Path,
    index: &PortableIndex,
) -> Result<(), AppError> {
    for name in UPDATE_FILES {
        package::plain_file(&directory.join(name))?;
        let mut file = File::open(directory.join(name))
            .map_err(|_| invalid())?
            .take(MAX_FILE + 1);
        let mut hash = Sha256::new();
        let size = std::io::copy(&mut file, &mut hash).map_err(|_| invalid())?;
        if size > MAX_FILE || index.files.get(*name) != Some(&format!("{:x}", hash.finalize())) {
            return Err(invalid());
        }
    }
    let mut bytes = Vec::new();
    package::plain_file(&directory.join("checksums.json"))?;
    File::open(directory.join("checksums.json"))
        .map_err(|_| invalid())?
        .take(32769)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() > 32768
        || serde_json::from_slice::<PortableIndex>(&bytes).map_err(|_| invalid())? != *index
    {
        return Err(invalid());
    }
    validate_engine(directory, index)
}

impl PortableIndex {
    fn validate(&self, manifest: &SoftwareManifest) -> Result<(), AppError> {
        if self.schema_version != 1
            || self.version != manifest.version
            || self.target != "windows-x86_64"
            || self.profile != "release"
            || self.core_version != manifest.core_version
            || Version::parse(&self.core_version).is_err()
            || self.engine_revision == 0
            || self.engine_revision > rela_manifests::MAX_REVISION
            || self.files.len() != UPDATE_FILES.len()
            || !UPDATE_FILES.iter().all(|path| {
                self.files
                    .get(*path)
                    .is_some_and(|value| digest_valid(value))
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
}

struct ArchiveIndex {
    index: PortableIndex,
    indices: BTreeMap<String, usize>,
    bytes: Vec<u8>,
    expanded: u64,
}

fn inspect_archive<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest: &SoftwareManifest,
) -> Result<ArchiveIndex, AppError> {
    if archive.len() > 128 || archive.len() < UPDATE_FILES.len() + 1 {
        return Err(invalid());
    }
    let expected_root = format!("Rela_{}_x64-update", manifest.version);
    let mut indices = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut expanded = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|_| invalid())?;
        if file.encrypted()
            || file.is_symlink()
            || !matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
            || file.name_raw() != file.name().as_bytes()
        {
            return Err(invalid());
        }
        let name = file.name();
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(invalid());
        }
        let relative = name
            .strip_prefix(&format!("{expected_root}/"))
            .ok_or_else(invalid)?;
        if file.is_dir() {
            if !matches!(relative, "" | "easytier/" | "third-party-licenses/") || file.size() != 0 {
                return Err(invalid());
            }
            continue;
        }
        if relative != "checksums.json" && !UPDATE_FILES.contains(&relative) {
            return Err(invalid());
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
        {
            return Err(invalid());
        }
        if file.size() > MAX_FILE
            || (matches!(relative, "checksums.json" | "easytier/manifest.json")
                && file.size() > 32768)
        {
            return Err(invalid());
        }
        expanded = expanded.checked_add(file.size()).ok_or_else(invalid)?;
        if expanded > MAX_EXPANDED {
            return Err(invalid());
        }
        indices.insert(relative.to_string(), index);
    }
    if indices.len() != UPDATE_FILES.len() + 1 {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    archive
        .by_index(*indices.get("checksums.json").ok_or_else(invalid)?)
        .map_err(|_| invalid())?
        .take(32769)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() > 32768 {
        return Err(invalid());
    }
    let index: PortableIndex = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    index.validate(manifest)?;

    Ok(ArchiveIndex {
        index,
        indices,
        bytes,
        expanded,
    })
}

pub fn stage(
    package: &mut VerifiedPackage,
    manifest: &SoftwareManifest,
    destination: &Path,
) -> Result<PortableStage, AppError> {
    manifest
        .validate(manifest.channel)
        .map_err(manifest_error)?;
    if !package.matches(&manifest.portable) {
        return Err(invalid());
    }
    let mut archive = ZipArchive::new(package.reader()?).map_err(|_| invalid())?;
    let ArchiveIndex {
        index,
        indices,
        bytes,
        expanded,
    } = inspect_archive(&mut archive, manifest)?;

    let parent = destination.parent().ok_or_else(invalid)?;
    package::plain_directory(parent)?;
    if fs2::available_space(parent).map_err(|_| invalid())?
        < expanded.saturating_add(32 * 1024 * 1024)
    {
        return Err(AppError::new(
            "update_space_insufficient",
            "更新暂存目录空间不足，请释放空间后重试。",
        ));
    }
    // Must be a new directory. No existing program or data file is ever opened for writing.
    fs::create_dir(destination).map_err(|_| invalid())?;
    let mut created = Vec::new();
    let result = (|| {
        for directory in ["easytier", "third-party-licenses"] {
            fs::create_dir(destination.join(directory)).map_err(|_| invalid())?;
        }
        for name in UPDATE_FILES {
            let mut file = archive
                .by_index(*indices.get(*name).ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
            let path = destination.join(name);
            let mut output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|_| invalid())?;
            created.push(path);
            let size = file.size();
            let mut hash = Sha256::new();
            let mut read = 0u64;
            let mut buffer = [0u8; 65536];
            loop {
                let count = file.read(&mut buffer).map_err(|_| invalid())?;
                if count == 0 {
                    break;
                }
                read += count as u64;
                if read > size || read > MAX_FILE {
                    return Err(invalid());
                }
                hash.update(&buffer[..count]);
                output.write_all(&buffer[..count]).map_err(|_| invalid())?;
            }
            if read != size || index.files.get(*name) != Some(&format!("{:x}", hash.finalize())) {
                return Err(invalid());
            }
            output.sync_all().map_err(|_| invalid())?;
        }
        validate_engine(destination, &index)?;
        for name in [
            "Rela.exe",
            "easytier/easytier-core.exe",
            "easytier/easytier-cli.exe",
        ] {
            verify_x64_pe(&destination.join(name))?;
        }
        let path = destination.join("checksums.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| invalid())?;
        created.push(path);
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| invalid())?;
        Ok(())
    })();
    if let Err(error) = result {
        for path in created.iter().rev() {
            let _ = fs::remove_file(path);
        }
        for directory in ["easytier", "third-party-licenses"] {
            let _ = fs::remove_dir(destination.join(directory));
        }
        let _ = fs::remove_dir(destination);
        return Err(error);
    }
    Ok(PortableStage {
        directory: destination.into(),
        index,
    })
}

/// Recover a persisted stage only by deriving its expected index from the signed ZIP again.
pub fn reopen_stage(
    package: &mut VerifiedPackage,
    manifest: &SoftwareManifest,
    directory: &Path,
) -> Result<PortableStage, AppError> {
    manifest
        .validate(manifest.channel)
        .map_err(manifest_error)?;
    if !package.matches(&manifest.portable) {
        return Err(invalid());
    }
    let mut archive = ZipArchive::new(package.reader()?).map_err(|_| invalid())?;
    let record = inspect_archive(&mut archive, manifest)?;
    let stage = PortableStage {
        directory: directory.into(),
        index: record.index,
    };
    stage.verify()?;
    for name in [
        "Rela.exe",
        "easytier/easytier-core.exe",
        "easytier/easytier-cli.exe",
    ] {
        verify_x64_pe(&directory.join(name))?;
    }
    Ok(stage)
}

fn validate_engine(root: &Path, index: &PortableIndex) -> Result<(), AppError> {
    #[derive(Deserialize)]
    struct Engine {
        version: String,
        engine_revision: u64,
        files: BTreeMap<String, String>,
    }
    let mut bytes = Vec::new();
    File::open(root.join("easytier/manifest.json"))
        .map_err(|_| invalid())?
        .take(32769)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() > 32768 {
        return Err(invalid());
    }
    let engine: Engine = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if engine.version != index.core_version
        || engine.engine_revision != index.engine_revision
        || engine.files.len() != 5
    {
        return Err(invalid());
    }
    for name in [
        "easytier-core.exe",
        "easytier-cli.exe",
        "wintun.dll",
        "Packet.dll",
        "WinDivert64.sys",
    ] {
        if engine.files.get(name) != index.files.get(&format!("easytier/{name}")) {
            return Err(invalid());
        }
    }
    Ok(())
}

pub fn verify_x64_pe(path: &Path) -> Result<(), AppError> {
    package::plain_file(path)?;
    let mut file = File::open(path).map_err(|_| invalid())?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos).map_err(|_| invalid())?;
    if &dos[..2] != b"MZ" {
        return Err(invalid());
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap());
    if !(64..=1024 * 1024).contains(&offset) {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(u64::from(offset)))
        .map_err(|_| invalid())?;
    let mut header = [0u8; 26];
    file.read_exact(&mut header).map_err(|_| invalid())?;
    if &header[..4] != b"PE\0\0"
        || u16::from_le_bytes(header[4..6].try_into().unwrap()) != 0x8664
        || u16::from_le_bytes(header[24..26].try_into().unwrap()) != 0x20b
    {
        return Err(AppError::new(
            "update_architecture_mismatch",
            "更新包不是 Windows x64 程序，原程序未更改。",
        ));
    }
    Ok(())
}
fn digest_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn invalid() -> AppError {
    AppError::new(
        "portable_package_invalid",
        "绿色更新包结构、版本或文件校验无效，原程序未更改。",
    )
}

#[cfg(test)]
pub(crate) mod tests;
