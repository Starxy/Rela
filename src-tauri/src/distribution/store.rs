//! Immutable cache/credential objects + one atomically replaced public state pointer.
//! A failed write may leave an unreferenced object, but cannot partially apply settings.
use super::manifest_error;
use crate::{
    network_config::{self, NetworkConfig},
    platform,
};
use fs2::FileExt;
use rela_manifests::{
    check_revision, sha256, verify_envelope, PublicKey, Purpose, ResourceManifest,
    MAX_ENVELOPE_BYTES,
};
use rela_protocol::{AppError, NetworkConfigUpdate, NetworkConfigView};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct ConfigStore {
    root: PathBuf,
    keys: Vec<PublicKey>,
    transaction: std::sync::Mutex<()>,
}

struct ConfigLock<'a> {
    _file: File,
    _guard: std::sync::MutexGuard<'a, ()>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LocalSettings {
    network_name: String,
    peers: Vec<String>,
    private_mode: bool,
    disable_p2p: bool,
    gateway_ip: Option<String>,
}
impl Default for LocalSettings {
    fn default() -> Self {
        Self {
            network_name: String::new(),
            peers: vec![],
            private_mode: true,
            disable_p2p: true,
            gateway_ip: None,
        }
    }
}
impl LocalSettings {
    fn from_config(config: &NetworkConfig) -> Self {
        Self {
            network_name: config.network_name.clone(),
            peers: config.peers.clone(),
            private_mode: config.private_mode,
            disable_p2p: config.disable_p2p,
            gateway_ip: config.gateway_ip.clone(),
        }
    }
    fn with_secret(&self, credential_secret: String) -> NetworkConfig {
        NetworkConfig {
            network_name: self.network_name.clone(),
            peers: self.peers.clone(),
            private_mode: self.private_mode,
            disable_p2p: self.disable_p2p,
            gateway_ip: self.gateway_ip.clone(),
            credential_secret,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialRef {
    id: String,
    network: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    network: String,
    secret: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheRef {
    envelope_digest: String,
    payload_digest: String,
    version: u64,
    etag: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Applied {
    local: LocalSettings,
    credential_id: Option<String>,
    resource_version: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema_version: u32,
    revision: u64,
    local: LocalSettings,
    local_override: bool,
    credential: Option<CredentialRef>,
    resources: Option<CacheRef>,
    applied: Option<Applied>,
    migrated_legacy: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            local: LocalSettings::default(),
            local_override: false,
            credential: None,
            resources: None,
            applied: None,
            migrated_legacy: false,
        }
    }
}

pub type SyncStatus = rela_protocol::ResourceSyncStatus;

pub struct RefreshTicket {
    revision: u64,
    manual: bool,
    pub etag: Option<String>,
    pub cached_resources: Option<Vec<u8>>,
}

pub struct ConnectionSnapshot {
    pub config: NetworkConfig,
    applied: Applied,
}

impl ConfigStore {
    pub fn new(root: PathBuf, keys: Vec<PublicKey>) -> Self {
        Self {
            root,
            keys,
            transaction: std::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> Result<ConfigLock<'_>, AppError> {
        // Concurrent commands in this process wait briefly for their own transaction;
        // the OS lock still excludes other processes sharing this configuration directory.
        let guard = self.transaction.lock().map_err(|_| state_error())?;
        fs::create_dir_all(&self.root).map_err(|_| platform::storage_error())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join("configuration.lock"))
            .map_err(|_| platform::storage_error())?;
        file.try_lock_exclusive().map_err(|_| {
            AppError::new(
                "configuration_busy",
                "另一份 Rela 正在保存配置，请稍后重试。",
            )
        })?;
        Ok(ConfigLock {
            _file: file,
            _guard: guard,
        })
    }

    fn read_state(&self) -> Result<State, AppError> {
        match read_bounded(&self.root.join("configuration.json"), 64 * 1024) {
            Ok(bytes) => {
                let state: State = serde_json::from_slice(&bytes).map_err(|_| state_error())?;
                if state.schema_version != 1 {
                    return Err(state_error());
                }
                if !state.local.network_name.is_empty() || !state.local.peers.is_empty() {
                    state
                        .local
                        .with_secret(String::new())
                        .normalize_and_validate(false)?;
                }
                if let Some(credential) = &state.credential {
                    if !safe_id(&credential.id) {
                        return Err(state_error());
                    }
                }
                if let Some(cache) = &state.resources {
                    if !is_digest(&cache.envelope_digest) || !is_digest(&cache.payload_digest) {
                        return Err(state_error());
                    }
                    check_revision(cache.version, &cache.payload_digest, None)
                        .map_err(manifest_error)?;
                }
                Ok(state)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => self.migrate_legacy(),
            Err(_) => Err(state_error()),
        }
    }

    fn migrate_legacy(&self) -> Result<State, AppError> {
        let legacy = self.root.join("network.dat");
        if !legacy.try_exists().map_err(|_| platform::storage_error())? {
            return Ok(State::default());
        }
        let config = network_config::load(&legacy)?;
        // Existing explicitly saved routes are preserved until a manual online refresh.
        let mut state = State {
            local: LocalSettings::from_config(&config),
            local_override: true,
            migrated_legacy: true,
            ..State::default()
        };
        if !config.credential_secret.is_empty() {
            state.credential =
                Some(self.write_credential(&config.network_name, &config.credential_secret)?);
        }
        self.commit(&mut state)?;
        // The original encrypted file remains as the migration backup until service acceptance.
        Ok(state)
    }

    fn commit(&self, state: &mut State) -> Result<(), AppError> {
        state.revision = state.revision.checked_add(1).ok_or_else(state_error)?;
        let bytes = serde_json::to_vec_pretty(state).map_err(|_| state_error())?;
        platform::atomic_write(&self.root.join("configuration.json"), &bytes)
    }

    fn credential(&self, state: &State) -> (String, bool) {
        let Some(reference) = &state.credential else {
            return (String::new(), false);
        };
        if reference.network != state.local.network_name {
            return (String::new(), true);
        }
        let result = (|| {
            let encrypted = read_bounded(
                &self
                    .root
                    .join("credentials")
                    .join(format!("{}.dat", reference.id)),
                16 * 1024,
            )
            .map_err(|_| state_error())?;
            let plain = platform::unprotect(&encrypted)?;
            let credential: Credential =
                serde_json::from_slice(&plain).map_err(|_| state_error())?;
            if credential.network != reference.network {
                return Err(state_error());
            }
            let mut config = state.local.with_secret(credential.secret);
            config.normalize_and_validate(true)?;
            Ok(config.credential_secret)
        })();
        match result {
            Ok(secret) => (secret, false),
            Err(_) => (String::new(), true),
        }
    }

    fn write_credential(&self, network: &str, secret: &str) -> Result<CredentialRef, AppError> {
        let id = unique_id();
        let plain = serde_json::to_vec(&Credential {
            network: network.into(),
            secret: secret.into(),
        })
        .map_err(|_| state_error())?;
        let encrypted = platform::protect(&plain)?;
        platform::atomic_write(
            &self.root.join("credentials").join(format!("{id}.dat")),
            &encrypted,
        )?;
        Ok(CredentialRef {
            id,
            network: network.into(),
        })
    }

    fn cache(&self, state: &State) -> Result<Option<(Vec<u8>, ResourceManifest)>, AppError> {
        let Some(cache) = &state.resources else {
            return Ok(None);
        };
        let bytes = read_bounded(
            &self
                .root
                .join("cache")
                .join(format!("resources-{}.json", cache.envelope_digest)),
            MAX_ENVELOPE_BYTES,
        )
        .map_err(|_| manifest_error("资源缓存损坏，请手动更新线上配置。"))?;
        if sha256(&bytes) != cache.envelope_digest {
            return Err(manifest_error("资源缓存校验失败，请手动更新线上配置。"));
        }
        let payload = if ResourceManifest::parse(&bytes).is_ok() {
            bytes
        } else {
            // Keep previously verified signed caches readable during the move to resources.json.
            // New downloads are parsed directly and never use this compatibility path.
            verify_envelope(&bytes, Purpose::Resources, &self.keys).map_err(manifest_error)?
        };
        if sha256(&payload) != cache.payload_digest {
            return Err(manifest_error("资源缓存内容不匹配。"));
        }
        let manifest = ResourceManifest::parse(&payload).map_err(manifest_error)?;
        if manifest.version != cache.version {
            return Err(manifest_error("资源缓存版本不匹配。"));
        }
        Ok(Some((payload, manifest)))
    }

    pub fn configuration(&self) -> Result<NetworkConfigView, AppError> {
        let _lock = self.lock()?;
        let state = self.read_state()?;
        Ok(state.local.with_secret(self.credential(&state).0).view())
    }

    pub fn status(&self) -> Result<SyncStatus, AppError> {
        let _lock = self.lock()?;
        self.status_for(&self.read_state()?)
    }

    fn status_for(&self, state: &State) -> Result<SyncStatus, AppError> {
        let (secret, unavailable) = self.credential(state);
        let credential_id = state.credential.as_ref().map(|credential| &credential.id);
        Ok(SyncStatus {
            local_override: state.local_override,
            resource_version: state.resources.as_ref().map(|cache| cache.version),
            applied_resource_version: state
                .applied
                .as_ref()
                .and_then(|applied| applied.resource_version),
            pending_reconnect: state.applied.as_ref().is_some_and(|applied| {
                applied.local != state.local || applied.credential_id.as_ref() != credential_id
            }),
            configuration_ready: !state.local.network_name.is_empty()
                && !state.local.peers.is_empty(),
            credential_unavailable: unavailable,
            has_credential: !secret.is_empty(),
            refreshing: false,
            last_checked: None,
            last_error: None,
        })
    }

    pub fn save(&self, update: NetworkConfigUpdate) -> Result<NetworkConfigView, AppError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        let old_config = state.local.with_secret(self.credential(&state).0);
        let credential_changed = update.credential_secret.is_some();
        let config = old_config.updated(update)?;
        if credential_changed {
            state.credential = if config.credential_secret.is_empty() {
                None
            } else {
                Some(self.write_credential(&config.network_name, &config.credential_secret)?)
            };
        } else if config.network_name != state.local.network_name {
            state.credential = None;
        }
        if config.network_name != state.local.network_name || config.peers != state.local.peers {
            // Once paused, only an explicit online refresh/reset opts back in.
            state.local_override |=
                self.cache(&state)
                    .ok()
                    .flatten()
                    .is_none_or(|(_, manifest)| {
                        config.network_name != manifest.network || config.peers != manifest.peer
                    });
        }
        state.local = LocalSettings::from_config(&config);
        self.commit(&mut state)?;
        Ok(config.view())
    }

    pub fn reset(&self) -> Result<NetworkConfigView, AppError> {
        let _lock = self.lock()?;
        let mut state = self.read_state().unwrap_or_default();
        let old_applied = state.applied.clone();
        let cache = self.cache(&state).ok().flatten();
        state.local = LocalSettings::default();
        if let Some((_, manifest)) = cache {
            state.local.network_name = manifest.network;
            state.local.peers = manifest.peer;
        }
        // Keep the previous resource cache reference; reset only affects local settings.
        state.credential = None;
        state.local_override = false;
        state.applied = old_applied;
        self.commit(&mut state)?;
        Ok(state.local.with_secret(String::new()).view())
    }

    pub fn begin_refresh(&self, manual: bool) -> Result<Option<RefreshTicket>, AppError> {
        let _lock = self.lock()?;
        let state = self.read_state()?;
        if state.local_override && !manual {
            return Ok(None);
        }
        let cached_resources = self.cache(&state).ok().flatten().map(|(bytes, _)| bytes);
        let etag = cached_resources.as_ref().and_then(|_| {
            let cache = state.resources.as_ref()?;
            // An ETag for the old signed document does not belong to the new plain file.
            (cache.envelope_digest == cache.payload_digest)
                .then(|| cache.etag.clone())
                .flatten()
        });
        Ok(Some(RefreshTicket {
            revision: state.revision,
            manual,
            etag,
            cached_resources,
        }))
    }

    pub fn apply_resources(
        &self,
        ticket: RefreshTicket,
        bytes: &[u8],
        etag: Option<String>,
    ) -> Result<SyncStatus, AppError> {
        let manifest = ResourceManifest::parse(bytes).map_err(manifest_error)?;
        let payload_digest = sha256(bytes);
        let envelope_digest = sha256(bytes);
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        if ticket.revision != state.revision || (!ticket.manual && state.local_override) {
            return Err(AppError::new(
                "configuration_changed",
                "配置已在获取期间修改，已保留本地设置，请重新更新。",
            ));
        }
        // The current contents of the fixed main/resources.json URL are authoritative.
        // Editing that file does not require a separate signing or revision publication step.
        platform::atomic_write(
            &self
                .root
                .join("cache")
                .join(format!("resources-{envelope_digest}.json")),
            bytes,
        )?;
        if state.local.network_name != manifest.network {
            state.credential = None;
        }
        state.local.network_name = manifest.network;
        state.local.peers = manifest.peer;
        state.local_override = false;
        state.resources = Some(CacheRef {
            envelope_digest,
            payload_digest,
            version: manifest.version,
            etag,
        });
        self.commit(&mut state)?;
        self.status_for(&state)
    }

    pub fn resources(&self) -> Result<Vec<rela_manifests::Resource>, AppError> {
        let _lock = self.lock()?;
        Ok(self
            .cache(&self.read_state()?)?
            .map(|(_, manifest)| manifest.resources)
            .unwrap_or_default())
    }

    pub fn connection_snapshot(&self) -> Result<ConnectionSnapshot, AppError> {
        let _lock = self.lock()?;
        let state = self.read_state()?;
        let (secret, unavailable) = self.credential(&state);
        if unavailable {
            return Err(AppError::new(
                "credential_unavailable",
                "此电脑或用户无法读取已存凭据，请在设置中重新填写。",
            ));
        }
        if state.local.network_name.is_empty() || state.local.peers.is_empty() {
            return Err(AppError::new(
                "configuration_unavailable",
                "等待获取网络配置，也可在设置中手动填写。",
            ));
        }
        let mut config = state.local.with_secret(secret);
        config.normalize_and_validate(true)?;
        Ok(ConnectionSnapshot {
            config,
            applied: Applied {
                local: state.local,
                credential_id: state.credential.map(|credential| credential.id),
                resource_version: state.resources.map(|cache| cache.version),
            },
        })
    }

    pub fn running_gateway(&self) -> Result<Option<String>, AppError> {
        let _lock = self.lock()?;
        let state = self.read_state()?;
        Ok(state
            .applied
            .map(|applied| applied.local.gateway_ip)
            .unwrap_or(state.local.gateway_ip))
    }

    pub fn mark_applied(&self, snapshot: ConnectionSnapshot) -> Result<(), AppError> {
        let _lock = self.lock()?;
        let mut state = self.read_state()?;
        state.applied = Some(snapshot.applied);
        self.commit(&mut state)
    }
}

fn state_error() -> AppError {
    AppError::new(
        "configuration_storage_failed",
        "本地配置无法读取或保存，请在设置中恢复默认。",
    )
}
fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
}
fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn unique_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{}-{stamp}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}
pub(super) fn read_bounded(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit as u64 {
        return Err(std::io::Error::new(ErrorKind::InvalidData, "invalid file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "file size limit",
        ));
    }
    Ok(bytes)
}

#[cfg(all(test, windows))]
mod tests;
