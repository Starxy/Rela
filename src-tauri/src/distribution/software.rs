//! Software discovery is independent of network configuration and local route overrides.
use super::{
    manifest_error,
    store::read_bounded,
    transport::{Fetcher, Response},
    DistributionConfig,
};
use crate::platform;
use fs2::FileExt;
use rela_manifests::{
    check_revision, sha256, verify_envelope, Channel, Purpose, SoftwareManifest, MAX_ENVELOPE_BYTES,
};
use rela_protocol::{AppError, InstallKind, SoftwareUpdateStatus, UpdateCandidate, UpdateChannel};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::ErrorKind,
    path::PathBuf,
    sync::{Mutex, MutexGuard},
};

pub struct SoftwareManager {
    root: PathBuf,
    config: DistributionConfig,
    fetcher: Fetcher,
    kind: InstallKind,
    current: Version,
    transaction: Mutex<()>,
    checking: tokio::sync::Mutex<()>,
    metadata: Mutex<Metadata>,
}

#[derive(Default)]
struct Metadata {
    last_checked: Option<String>,
    last_error: Option<String>,
    fresh: bool,
}

struct StoreLock<'a> {
    _file: File,
    _guard: MutexGuard<'a, ()>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Settings {
    channel: UpdateChannel,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheRef {
    revision: u64,
    version: Version,
    payload_digest: String,
    envelope_digest: String,
    etag: Option<String>,
}

pub struct SelectedUpdate {
    pub envelope: Vec<u8>,
    pub manifest: SoftwareManifest,
}

impl SoftwareManager {
    pub fn new(
        root: PathBuf,
        config: DistributionConfig,
        kind: InstallKind,
    ) -> Result<Self, AppError> {
        Ok(Self {
            root,
            config,
            fetcher: Fetcher::new()?,
            kind,
            current: Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| storage_error())?,
            transaction: Mutex::new(()),
            checking: tokio::sync::Mutex::new(()),
            metadata: Mutex::new(Metadata::default()),
        })
    }

    fn lock(&self) -> Result<StoreLock<'_>, AppError> {
        let guard = self.transaction.lock().map_err(|_| storage_error())?;
        fs::create_dir_all(&self.root).map_err(|_| platform::storage_error())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join("software.lock"))
            .map_err(|_| platform::storage_error())?;
        file.try_lock_exclusive()
            .map_err(|_| AppError::new("update_busy", "另一份 Rela 正在检查更新，请稍后重试。"))?;
        Ok(StoreLock {
            _file: file,
            _guard: guard,
        })
    }

    fn channel(&self) -> Result<UpdateChannel, AppError> {
        match read_bounded(&self.root.join("update-settings.json"), 1024) {
            Ok(bytes) => serde_json::from_slice::<Settings>(&bytes)
                .map(|settings| settings.channel)
                .map_err(|_| storage_error()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(UpdateChannel::Stable),
            Err(_) => Err(storage_error()),
        }
    }

    fn pointer_path(&self, channel: UpdateChannel) -> PathBuf {
        self.root
            .join(format!("software-{}.json", channel_name(channel)))
    }

    fn pointer(&self, channel: UpdateChannel) -> Result<Option<CacheRef>, AppError> {
        let bytes = match read_bounded(&self.pointer_path(channel), 4096) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(storage_error()),
        };
        let entry: CacheRef = serde_json::from_slice(&bytes).map_err(|_| storage_error())?;
        if ![&entry.envelope_digest, &entry.payload_digest]
            .iter()
            .all(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
        {
            return Err(storage_error());
        }
        check_revision(entry.revision, &entry.payload_digest, None).map_err(manifest_error)?;
        Ok(Some(entry))
    }

    fn cache(
        &self,
        channel: UpdateChannel,
        pointer: &CacheRef,
    ) -> Result<(Vec<u8>, SoftwareManifest), AppError> {
        let bytes = read_bounded(
            &self
                .root
                .join("cache")
                .join(format!("software-{}.json", pointer.envelope_digest)),
            MAX_ENVELOPE_BYTES,
        )
        .map_err(|_| manifest_error("软件更新缓存不可用，请重新检查更新。"))?;
        if sha256(&bytes) != pointer.envelope_digest {
            return Err(manifest_error("软件更新缓存校验失败。"));
        }
        let payload = verify_envelope(&bytes, Purpose::Software, &self.config.keys)
            .map_err(manifest_error)?;
        let manifest =
            SoftwareManifest::parse(&payload, manifest_channel(channel)).map_err(manifest_error)?;
        if sha256(&payload) != pointer.payload_digest
            || manifest.revision != pointer.revision
            || manifest.version != pointer.version
        {
            return Err(manifest_error("软件更新缓存内容不匹配。"));
        }
        Ok((bytes, manifest))
    }

    fn accept(
        &self,
        channel: UpdateChannel,
        bytes: &[u8],
        etag: Option<String>,
    ) -> Result<(), AppError> {
        let payload =
            verify_envelope(bytes, Purpose::Software, &self.config.keys).map_err(manifest_error)?;
        let manifest =
            SoftwareManifest::parse(&payload, manifest_channel(channel)).map_err(manifest_error)?;
        let payload_digest = sha256(&payload);
        let envelope_digest = sha256(bytes);
        let _lock = self.lock()?;
        // Re-read the floor inside the lock: another process may have accepted a newer feed.
        let previous = self.pointer(channel)?;
        check_revision(
            manifest.revision,
            &payload_digest,
            previous
                .as_ref()
                .map(|entry| (entry.revision, entry.payload_digest.as_str())),
        )
        .map_err(manifest_error)?;
        if previous
            .as_ref()
            .is_some_and(|entry| manifest.version < entry.version)
        {
            return Err(manifest_error("软件清单不能退回已接受版本之前。"));
        }
        if let Some(previous) = &previous {
            if manifest.version == previous.version && payload_digest != previous.payload_digest {
                let (_, old) = self.cache(channel, previous)?;
                if manifest.installer != old.installer
                    || manifest.portable != old.portable
                    || manifest.core_version != old.core_version
                    || manifest.minimum_app_version != old.minimum_app_version
                {
                    return Err(manifest_error(
                        "同一软件版本的发布包和兼容要求不能改变，请发布新版本。",
                    ));
                }
            }
        }
        platform::atomic_write(
            &self
                .root
                .join("cache")
                .join(format!("software-{envelope_digest}.json")),
            bytes,
        )?;
        let pointer = CacheRef {
            revision: manifest.revision,
            version: manifest.version,
            payload_digest,
            envelope_digest,
            etag,
        };
        platform::atomic_write(
            &self.pointer_path(channel),
            &serde_json::to_vec_pretty(&pointer).map_err(|_| storage_error())?,
        )
    }

    pub fn status(&self) -> Result<SoftwareUpdateStatus, AppError> {
        let _lock = self.lock()?;
        let channel = self.channel()?;
        let candidate = self
            .pointer(channel)?
            .map(|pointer| self.cache(channel, &pointer).map(|(_, manifest)| manifest))
            .transpose()?;
        let candidate = candidate
            .filter(|manifest| manifest.version > self.current)
            .map(|manifest| UpdateCandidate {
                version: manifest.version.to_string(),
                notes: manifest.notes,
                published_at: manifest.published_at,
                size: match self.kind {
                    InstallKind::Installer => manifest.installer.size + manifest.portable.size,
                    InstallKind::Portable => manifest.portable.size,
                },
                core_version: manifest.core_version,
                requires_manual_upgrade: self.current < manifest.minimum_app_version,
            });
        let metadata = self.metadata.lock().map_err(|_| storage_error())?;
        Ok(SoftwareUpdateStatus {
            channel,
            install_kind: self.kind,
            current_version: self.current.to_string(),
            checking: self.checking.try_lock().is_err(),
            last_checked: metadata.last_checked.clone(),
            last_error: metadata.last_error.clone(),
            cached: candidate.is_some() && !metadata.fresh,
            candidate,
        })
    }

    /// Bind confirmation to the exact version currently displayed in the UI.
    /// The helper independently verifies the returned envelope and package again.
    pub fn select(&self, displayed_version: &str) -> Result<SelectedUpdate, AppError> {
        let _lock = self.lock()?;
        let channel = self.channel()?;
        let pointer = self
            .pointer(channel)?
            .ok_or_else(|| manifest_error("请先检查软件更新。"))?;
        let (envelope, manifest) = self.cache(channel, &pointer)?;
        if manifest.version.to_string() != displayed_version || manifest.version <= self.current {
            return Err(manifest_error("更新信息已改变，请重新检查并确认版本。"));
        }
        if self.current < manifest.minimum_app_version {
            return Err(AppError::new(
                "manual_upgrade_required",
                "当前版本需要先手动升级。",
            ));
        }
        Ok(SelectedUpdate { envelope, manifest })
    }

    pub fn set_channel(&self, channel: UpdateChannel) -> Result<SoftwareUpdateStatus, AppError> {
        let guard = self
            .checking
            .try_lock()
            .map_err(|_| AppError::new("update_busy", "正在检查更新，请稍候再切换渠道。"))?;
        {
            let _lock = self.lock()?;
            platform::atomic_write(
                &self.root.join("update-settings.json"),
                &serde_json::to_vec(&Settings { channel }).map_err(|_| storage_error())?,
            )?;
            *self.metadata.lock().map_err(|_| storage_error())? = Metadata::default();
        }
        drop(guard);
        self.status()
    }

    pub async fn check(&self) -> Result<SoftwareUpdateStatus, AppError> {
        let guard = self
            .checking
            .try_lock()
            .map_err(|_| AppError::new("update_busy", "正在检查软件更新，请稍候。"))?;
        let result = async {
            let (channel, cached, etag) = {
                let _lock = self.lock()?;
                let channel = self.channel()?;
                let pointer = self.pointer(channel)?;
                let cached = pointer
                    .as_ref()
                    .and_then(|entry| self.cache(channel, entry).ok().map(|(bytes, _)| bytes));
                let etag = cached.as_ref().and_then(|_| pointer.as_ref()?.etag.clone());
                (channel, cached, etag)
            };
            let response = self
                .fetcher
                .fetch(
                    self.config.software_url(manifest_channel(channel)),
                    etag.as_deref(),
                    MAX_ENVELOPE_BYTES,
                )
                .await?;
            match response {
                Response::Content { bytes, etag } => self.accept(channel, &bytes, etag),
                Response::NotModified => self.accept(
                    channel,
                    &cached.ok_or_else(|| manifest_error("软件更新缓存不可用，请重新检查。"))?,
                    etag,
                ),
            }
        }
        .await;
        {
            let mut metadata = self.metadata.lock().map_err(|_| storage_error())?;
            metadata.last_checked = Some(chrono::Utc::now().to_rfc3339());
            metadata.last_error = result
                .as_ref()
                .err()
                .map(|error: &AppError| error.message.clone());
            metadata.fresh = result.is_ok();
        }
        drop(guard);
        result?;
        self.status()
    }
}

fn channel_name(channel: UpdateChannel) -> &'static str {
    match channel {
        UpdateChannel::Stable => "stable",
        UpdateChannel::Test => "test",
    }
}
fn manifest_channel(channel: UpdateChannel) -> Channel {
    match channel {
        UpdateChannel::Stable => Channel::Stable,
        UpdateChannel::Test => Channel::Test,
    }
}
fn storage_error() -> AppError {
    AppError::new(
        "update_storage_failed",
        "无法读取或保存软件更新状态，请检查应用数据目录。",
    )
}

#[cfg(test)]
mod tests;
