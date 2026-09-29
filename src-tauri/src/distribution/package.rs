//! Verified updater artifacts. The open handle prevents Windows writers/replacement
//! until the caller finishes using the verified bytes; partial files are disposable.
use super::transport::Fetcher;
use crate::platform;
use base64::{engine::general_purpose::STANDARD, Engine};
use minisign_verify::{PublicKey, Signature};
use rela_manifests::Package;
use rela_protocol::AppError;
use reqwest::{header, StatusCode};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use url::Url;

pub const MAX_PACKAGE_SIZE: u64 = 2 * 1024 * 1024 * 1024;

pub struct VerifiedPackage {
    path: PathBuf,
    file: Option<File>,
    temporary: bool,
    digest: String,
    size: u64,
}
impl VerifiedPackage {
    /// Reverify both signatures before trusting the version in the minisign
    /// trusted comment. The comment is part of minisign's global signature.
    pub fn open_versioned(
        path: &Path,
        package: &Package,
        public_key: &str,
        version: &semver::Version,
    ) -> Result<Self, AppError> {
        let verified = Self::open(path, package, public_key)?;
        let (_, signature) = signature_inputs(public_key, &package.signature)?;
        let mut versions = signature
            .trusted_comment()
            .split('\t')
            .filter_map(|field| field.strip_prefix("version:"));
        let matches = versions
            .next()
            .and_then(|value| semver::Version::parse(value.strip_prefix('v').unwrap_or(value)).ok())
            .is_some_and(|signed| &signed == version);
        if !matches || versions.next().is_some() {
            return Err(AppError::new(
                "package_version_mismatch",
                "更新包签名中的版本与确认版本不一致。",
            ));
        }
        Ok(verified)
    }

    /// Reverify on every reopen; the path alone is never a verification token.
    pub fn open(path: &Path, package: &Package, public_key: &str) -> Result<Self, AppError> {
        plain_file(path)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let file = options.open(path).map_err(|_| storage_error())?;
        let mut value = Self {
            path: path.into(),
            file: Some(file),
            temporary: false,
            digest: package.sha256.clone(),
            size: package.size,
        };
        value.verify(package, public_key)?;
        Ok(value)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn matches(&self, package: &Package) -> bool {
        self.digest == package.sha256 && self.size == package.size
    }
    pub fn reader(&mut self) -> Result<impl Read + Seek + '_, AppError> {
        let file = self.file.as_mut().ok_or_else(storage_error)?;
        file.seek(SeekFrom::Start(0)).map_err(|_| storage_error())?;
        Ok(file)
    }
    /// Preserve a verified artifact for the helper, which verifies it again on reopen.
    pub fn persist_copy(
        &mut self,
        target: &Path,
        package: &Package,
        public_key: &str,
    ) -> Result<Self, AppError> {
        if !self.matches(package) {
            return Err(integrity_error());
        }
        plain_directory(target.parent().ok_or_else(storage_error)?)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(|_| storage_error())?;
        let copied = std::io::copy(&mut self.reader()?.take(package.size + 1), &mut output)
            .map_err(|_| storage_error())?;
        output.sync_all().map_err(|_| storage_error())?;
        drop(output);
        if copied != package.size {
            return Err(integrity_error());
        }
        Self::open(target, package, public_key)
    }
    fn verify(&mut self, package: &Package, public_key: &str) -> Result<(), AppError> {
        validate_package(package)?;
        let (key, signature) = signature_inputs(public_key, &package.signature)?;
        let mut verifier = key
            .verify_stream(&signature)
            .map_err(|_| signature_error())?;
        if self
            .file
            .as_ref()
            .ok_or_else(storage_error)?
            .metadata()
            .map_err(|_| storage_error())?
            .len()
            != package.size
        {
            return Err(integrity_error());
        }
        let mut file = self.reader()?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut bytes = [0u8; 65536];
        loop {
            let count = file.read(&mut bytes).map_err(|_| storage_error())?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > package.size {
                return Err(integrity_error());
            }
            hash.update(&bytes[..count]);
            verifier.update(&bytes[..count]);
        }
        if total != package.size || format!("{:x}", hash.finalize()) != package.sha256 {
            return Err(integrity_error());
        }
        verifier.finalize().map_err(|_| signature_error())?;
        drop(file);
        self.reader()?;
        Ok(())
    }
}
impl Drop for VerifiedPackage {
    fn drop(&mut self) {
        drop(self.file.take());
        if self.temporary {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Fetcher {
    /// Only called with a package selected from a verified software manifest.
    pub async fn download_package(
        &self,
        package: &Package,
        public_key: &str,
        directory: &Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<VerifiedPackage, AppError> {
        validate_package(package)?;
        // Parse both keys before any network/disk work, rejecting legacy non-streamable signatures.
        let (key, signature) = signature_inputs(public_key, &package.signature)?;
        key.verify_stream(&signature)
            .map_err(|_| signature_error())?;
        let url = Url::parse(&package.url).map_err(|_| integrity_error())?;
        if !super::transport::allowed_url(&url, self.allow_loopback) {
            return Err(download_error());
        }
        let mut response = self
            .client
            .get(url)
            .header(header::ACCEPT, "application/octet-stream")
            .send()
            .await
            .map_err(|_| download_error())?;
        if response.status() != StatusCode::OK
            || response
                .content_length()
                .is_some_and(|size| size != package.size)
        {
            return Err(download_error());
        }
        fs::create_dir_all(directory).map_err(|_| storage_error())?;
        plain_directory(directory)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| storage_error())?
            .as_nanos();
        let path = directory.join(format!("package-{}-{stamp}.part", std::process::id()));
        let mut options = OpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let file = options.open(&path).map_err(|_| storage_error())?;
        let mut result = VerifiedPackage {
            path,
            file: Some(file),
            temporary: true,
            digest: package.sha256.clone(),
            size: package.size,
        };
        let mut total = 0u64;
        progress(0, package.size);
        while let Some(chunk) = response.chunk().await.map_err(|_| download_error())? {
            total = total
                .checked_add(chunk.len() as u64)
                .ok_or_else(integrity_error)?;
            if total > package.size {
                return Err(integrity_error());
            }
            result
                .file
                .as_mut()
                .ok_or_else(storage_error)?
                .write_all(&chunk)
                .map_err(|_| storage_error())?;
            progress(total, package.size);
        }
        result
            .file
            .as_ref()
            .ok_or_else(storage_error)?
            .sync_all()
            .map_err(|_| storage_error())?;
        result.verify(package, public_key)?;
        Ok(result)
    }
}

pub fn validate_public_key(value: &str) -> Result<(), AppError> {
    PublicKey::decode(&decode(value)?)
        .map_err(|_| signature_error())
        .map(|_| ())
}
fn signature_inputs(key: &str, signature: &str) -> Result<(PublicKey, Signature), AppError> {
    Ok((
        PublicKey::decode(&decode(key)?).map_err(|_| signature_error())?,
        Signature::decode(&decode(signature)?).map_err(|_| signature_error())?,
    ))
}
fn decode(value: &str) -> Result<String, AppError> {
    if value.len() > 4096 {
        return Err(signature_error());
    }
    String::from_utf8(STANDARD.decode(value).map_err(|_| signature_error())?)
        .map_err(|_| signature_error())
}
fn validate_package(package: &Package) -> Result<(), AppError> {
    if !(1..=MAX_PACKAGE_SIZE).contains(&package.size)
        || package.sha256.len() != 64
        || !package
            .sha256
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(integrity_error());
    }
    Ok(())
}
pub(crate) fn plain_directory(path: &Path) -> Result<(), AppError> {
    let metadata = plain_metadata(path)?;
    if !metadata.is_dir() {
        return Err(storage_error());
    }
    Ok(())
}
pub(crate) fn plain_file(path: &Path) -> Result<(), AppError> {
    if !plain_metadata(path)?.is_file() {
        return Err(storage_error());
    }
    Ok(())
}
fn plain_metadata(path: &Path) -> Result<fs::Metadata, AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| storage_error())?;
    for ancestor in path.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        let current = fs::symlink_metadata(ancestor).map_err(|_| storage_error())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if current.file_attributes() & 0x400 != 0 {
                return Err(storage_error());
            }
        }
        if current.file_type().is_symlink() || (ancestor != path && !current.is_dir()) {
            return Err(storage_error());
        }
    }
    Ok(metadata)
}
pub(super) fn integrity_error() -> AppError {
    AppError::new(
        "package_integrity_failed",
        "更新包内容或大小校验失败，原程序未更改。",
    )
}
fn signature_error() -> AppError {
    AppError::new("package_signature_failed", "更新包签名无效，原程序未更改。")
}
fn download_error() -> AppError {
    AppError::new(
        "package_download_failed",
        "更新包下载失败或中断，请重试。原程序未更改。",
    )
}
fn storage_error() -> AppError {
    platform::storage_error()
}

#[cfg(test)]
pub(crate) mod tests;
