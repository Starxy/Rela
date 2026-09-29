use super::{
    store::ConfigStore,
    transport::{Fetcher, Response},
    DistributionConfig,
};
use crate::platform;
use rela_manifests::{Resource, ResourceKind, MAX_ENVELOPE_BYTES};
use rela_protocol::{AppError, Availability, LabResource, ResourceSyncStatus};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpStream,
    sync::{Mutex, Semaphore},
    task::JoinSet,
    time::timeout,
};

pub struct ResourceManager {
    pub store: Arc<ConfigStore>,
    config: DistributionConfig,
    fetcher: Fetcher,
    refresh_lock: Mutex<()>,
    probe_lock: Mutex<()>,
    metadata: StdMutex<Metadata>,
    probes: StdMutex<Option<ProbeCache>>,
}
#[derive(Default)]
struct Metadata {
    last_checked: Option<String>,
    last_error: Option<String>,
}
struct ProbeCache {
    fingerprint: String,
    at: Instant,
    resources: Vec<LabResource>,
}

impl ResourceManager {
    pub fn new(root: PathBuf, config: DistributionConfig) -> Result<Self, AppError> {
        Ok(Self {
            store: Arc::new(ConfigStore::new(root, config.keys.clone())),
            config,
            fetcher: Fetcher::new()?,
            refresh_lock: Mutex::new(()),
            probe_lock: Mutex::new(()),
            metadata: StdMutex::new(Metadata::default()),
            probes: StdMutex::new(None),
        })
    }

    pub fn status(&self) -> Result<ResourceSyncStatus, AppError> {
        let mut status = self.store.status()?;
        let metadata = self.metadata.lock().map_err(|_| internal())?;
        status.last_checked = metadata.last_checked.clone();
        status.last_error = metadata.last_error.clone();
        status.refreshing = self.refresh_lock.try_lock().is_err();
        Ok(status)
    }

    pub async fn refresh(&self, manual: bool) -> Result<ResourceSyncStatus, AppError> {
        let guard = self
            .refresh_lock
            .try_lock()
            .map_err(|_| AppError::new("refresh_busy", "正在获取线上配置，请稍候。"))?;
        let Some(ticket) = self.store.begin_refresh(manual)? else {
            drop(guard);
            return self.status();
        };
        let result = async {
            match self
                .fetcher
                .fetch(
                    &self.config.resources_url,
                    ticket.etag.as_deref(),
                    MAX_ENVELOPE_BYTES,
                )
                .await?
            {
                Response::NotModified => {
                    let bytes = ticket.cached_envelope.clone().ok_or_else(|| {
                        AppError::new("invalid_manifest", "没有已验证缓存，请重试。")
                    })?;
                    let etag = ticket.etag.clone();
                    self.store.apply_resources(ticket, &bytes, etag)
                }
                Response::Content { bytes, etag } => {
                    self.store.apply_resources(ticket, &bytes, etag)
                }
            }
        }
        .await;
        {
            let mut metadata = self.metadata.lock().map_err(|_| internal())?;
            metadata.last_checked = Some(chrono::Utc::now().to_rfc3339());
            metadata.last_error = result.as_ref().err().map(|error| error.message.clone());
        }
        drop(guard);
        result?;
        self.status()
    }

    pub async fn resources(&self, connected: bool) -> Result<Vec<LabResource>, AppError> {
        let entries = self.store.resources()?;
        if !connected {
            *self.probes.lock().map_err(|_| internal())? = None;
            return Ok(entries
                .iter()
                .map(|resource| view(resource, Availability::Unknown))
                .collect());
        }
        let _guard = self.probe_lock.lock().await;
        let fingerprint =
            rela_manifests::sha256(&serde_json::to_vec(&entries).map_err(|_| internal())?);
        if let Some(cache) = &*self.probes.lock().map_err(|_| internal())? {
            if cache.fingerprint == fingerprint && cache.at.elapsed() < Duration::from_secs(4) {
                return Ok(cache.resources.clone());
            }
        }
        let permits = Arc::new(Semaphore::new(16));
        let mut tasks = JoinSet::new();
        for (index, resource) in entries.into_iter().enumerate() {
            let permits = Arc::clone(&permits);
            tasks.spawn(async move {
                let _permit = permits.acquire().await.map_err(|_| internal())?;
                let (host, port) = resource.probe_target().map_err(super::manifest_error)?;
                let availability = match timeout(
                    Duration::from_millis(1200),
                    TcpStream::connect((host.as_str(), port)),
                )
                .await
                {
                    Ok(Ok(_stream)) => Availability::Reachable,
                    _ => Availability::Unreachable,
                };
                Ok::<_, AppError>((index, view(&resource, availability)))
            });
        }
        let mut results = Vec::new();
        while let Some(result) = tasks.join_next().await {
            results.push(result.map_err(|_| internal())??);
        }
        results.sort_by_key(|(index, _)| *index);
        let resources: Vec<_> = results.into_iter().map(|(_, value)| value).collect();
        *self.probes.lock().map_err(|_| internal())? = Some(ProbeCache {
            fingerprint,
            at: Instant::now(),
            resources: resources.clone(),
        });
        Ok(resources)
    }

    pub fn counts(&self, connected: bool) -> Result<(u32, u32), AppError> {
        let entries = self.store.resources()?;
        let fingerprint =
            rela_manifests::sha256(&serde_json::to_vec(&entries).map_err(|_| internal())?);
        let available = self
            .probes
            .lock()
            .map_err(|_| internal())?
            .as_ref()
            .filter(|cache| {
                connected
                    && cache.fingerprint == fingerprint
                    && cache.at.elapsed() < Duration::from_secs(10)
            })
            .map(|cache| {
                cache
                    .resources
                    .iter()
                    .filter(|resource| matches!(resource.availability, Availability::Reachable))
                    .count()
            })
            .unwrap_or(0);
        Ok((available as u32, entries.len() as u32))
    }

    pub fn open(&self, id: &str) -> Result<(), AppError> {
        let resource = self
            .store
            .resources()?
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| AppError::new("resource_unavailable", "资源不存在或已从清单移除。"))?;
        resource.validate().map_err(super::manifest_error)?;
        platform::open_resource(&resource)
    }
}

fn view(resource: &Resource, availability: Availability) -> LabResource {
    let kind = match resource.kind {
        ResourceKind::Ssh => rela_protocol::ResourceKind::Ssh,
        ResourceKind::Web => rela_protocol::ResourceKind::Web,
        ResourceKind::Nas => rela_protocol::ResourceKind::Nas,
    };
    let address = if matches!(resource.kind, ResourceKind::Ssh) {
        let host = if resource.address.contains(':') {
            format!("[{}]", resource.address)
        } else {
            resource.address.clone()
        };
        format!("{}:{}", host, resource.port.expect("validated SSH port"))
    } else {
        resource.address.clone()
    };
    LabResource {
        id: resource.id.clone(),
        name: resource.name.clone(),
        kind,
        description: resource.description.clone(),
        address,
        availability,
    }
}
fn internal() -> AppError {
    AppError::new("internal_error", "资源操作未完成，请重试。")
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use rela_manifests::{signing_message, PublicKey, Purpose, SignedEnvelope};
    use std::{
        fs,
        net::TcpListener,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn actual_tcp_probes_report_reachable_unreachable_and_offline_unknown() {
        let root = std::env::temp_dir().join(format!(
            "rela-probe-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let key = SigningKey::from_bytes(&[17; 32]);
        let mut config = DistributionConfig::bundled().unwrap();
        config.keys = vec![PublicKey {
            id: "probe-test-only".into(),
            purpose: Purpose::Resources,
            public_key: STANDARD.encode(key.verifying_key().to_bytes()),
        }];
        let manager = ResourceManager::new(root.clone(), config).unwrap();
        let open = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);
        let payload = serde_json::to_vec(&serde_json::json!({ "schema_version": 1, "version": 1, "network": "isolated-probe",
            "peer": ["tcp://127.0.0.1:1111"], "resources": [
                { "id": "open", "name": "Open", "kind": "ssh", "address": "127.0.0.1", "port": open.local_addr().unwrap().port(), "description": "" },
                { "id": "closed", "name": "Closed", "kind": "ssh", "address": "127.0.0.1", "port": closed_port, "description": "" }
            ] })).unwrap();
        let signature =
            key.sign(&signing_message(Purpose::Resources, "probe-test-only", &payload).unwrap());
        let envelope = serde_json::to_vec(&SignedEnvelope {
            format: "rela.signed.v1".into(),
            key_id: "probe-test-only".into(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signature.to_bytes()),
        })
        .unwrap();
        manager
            .store
            .apply_resources(
                manager.store.begin_refresh(true).unwrap().unwrap(),
                &envelope,
                None,
            )
            .unwrap();
        tauri::async_runtime::block_on(async {
            let online = manager.resources(true).await.unwrap();
            assert!(matches!(online[0].availability, Availability::Reachable));
            assert!(matches!(online[1].availability, Availability::Unreachable));
            assert_eq!(manager.counts(true).unwrap(), (1, 2));
            let offline = manager.resources(false).await.unwrap();
            assert!(offline
                .iter()
                .all(|resource| matches!(resource.availability, Availability::Unknown)));
            assert_eq!(manager.counts(false).unwrap(), (0, 2));
            assert!(manager.open("not-in-manifest").is_err());
        });
        if root.parent() == Some(std::env::temp_dir().as_path())
            && root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("rela-probe-test-")
        {
            fs::remove_dir_all(root).unwrap();
        }
    }
}
