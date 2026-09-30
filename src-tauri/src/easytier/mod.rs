//! Rela 直接控制专用的 EasyTier Core Windows 服务，并通过官方 CLI 读取本机 RPC。
mod deployment;
mod managed_service;
mod process;
mod status;

use crate::{
    distribution::store::ConfigStore,
    network_config::{self, NetworkConfig, RPC_PORTAL},
    platform::{self, ServiceState, SERVICE_NAME},
};
use rela_protocol::{
    AppError, ConnectionStatus, CoreState, NetworkConfigUpdate, NetworkConfigView, VersionInfo,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

pub use rela_protocol::EASYTIER_TARGET_VERSION;
const FILES: &[&str] = &[
    "easytier-core.exe",
    "easytier-cli.exe",
    "wintun.dll",
    "Packet.dll",
    "WinDivert64.sys",
];

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AssetManifest {
    engine_revision: u64,
    version: String,
    files: BTreeMap<String, String>,
}

impl AssetManifest {
    fn validate(&self) -> Result<(), AppError> {
        if self.engine_revision > rela_manifests::MAX_REVISION
            || semver::Version::parse(&self.version).is_err()
            || self.version.len() > 128
            || self.files.len() != FILES.len()
            || !FILES.iter().all(|name| {
                self.files.get(*name).is_some_and(|digest| {
                    digest.len() == 64
                        && digest
                            .bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
            })
        {
            return Err(integrity_error());
        }
        Ok(())
    }

    fn verify(&self, directory: &Path) -> Result<(), AppError> {
        self.validate()?;
        for name in FILES {
            let path = directory.join(name);
            let metadata = fs::symlink_metadata(&path).map_err(|_| integrity_error())?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(integrity_error());
                }
            }
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > 256 * 1024 * 1024
            {
                return Err(integrity_error());
            }
            let mut file = fs::File::open(path).map_err(|_| integrity_error())?;
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = file.read(&mut buffer).map_err(|_| integrity_error())?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            if self.files.get(*name) != Some(&format!("{:x}", hash.finalize())) {
                return Err(integrity_error());
            }
        }
        Ok(())
    }
}

pub struct EasyTierCore {
    configuration_store: Arc<ConfigStore>,
    binaries: PathBuf,
    requests: PathBuf,
    operations: Mutex<()>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreAction {
    Connect,
    Disconnect,
    Reconnect,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperRequest {
    action: CoreAction,
    config: Option<NetworkConfig>,
    device_name: String,
}

struct RequestFile(PathBuf);
impl Drop for RequestFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl EasyTierCore {
    pub fn new(
        config_dir: PathBuf,
        resource_dir: PathBuf,
        configuration_store: Arc<ConfigStore>,
    ) -> Self {
        Self {
            configuration_store,
            requests: config_dir.join("requests"),
            binaries: resource_dir.join("easytier"),
            operations: Mutex::new(()),
        }
    }

    pub async fn get_status(self: &Arc<Self>) -> Result<ConnectionStatus, AppError> {
        let this = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || this.status())
            .await
            .map_err(|_| internal_error())?
    }

    pub async fn control(
        self: &Arc<Self>,
        action: CoreAction,
        device_name: String,
    ) -> Result<ConnectionStatus, AppError> {
        let _guard = self.operations.lock().await;
        let this = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || {
            if !matches!(action, CoreAction::Disconnect) {
                verify_assets(&this.binaries)?;
            }
            let snapshot = if matches!(action, CoreAction::Disconnect) {
                None
            } else {
                Some(this.configuration_store.connection_snapshot()?)
            };
            let request = HelperRequest {
                action,
                config: snapshot.as_ref().map(|snapshot| snapshot.config.clone()),
                device_name,
            };
            let applied = if platform::is_elevated() {
                perform_helper(request, &this.binaries)?
            } else {
                fs::create_dir_all(&this.requests).map_err(|_| platform::storage_error())?;
                let stamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| internal_error())?
                    .as_nanos();
                let request_file = RequestFile(
                    this.requests
                        .join(format!("request-{}-{stamp}.bin", std::process::id())),
                );
                // Machine-scope DPAPI lets the approved elevated user read the request; the ACL limits file access.
                let bytes = platform::protect_request(
                    &serde_json::to_vec(&request).map_err(|_| internal_error())?,
                )?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&request_file.0)
                    .map_err(|_| platform::storage_error())?;
                platform::private_file(&request_file.0)?;
                file.write_all(&bytes)
                    .map_err(|_| platform::storage_error())?;
                file.sync_all().map_err(|_| platform::storage_error())?;
                drop(file);
                platform::elevate_helper(&request_file.0)?
            };
            if applied {
                if let Some(snapshot) = snapshot {
                    this.configuration_store.mark_applied(snapshot)?;
                }
            }
            this.status()
        })
        .await
        .map_err(|_| internal_error())?
    }

    pub async fn configuration(self: &Arc<Self>) -> Result<NetworkConfigView, AppError> {
        let _guard = self.operations.lock().await;
        self.configuration_store.configuration()
    }

    pub async fn save_configuration(
        self: &Arc<Self>,
        update: NetworkConfigUpdate,
    ) -> Result<NetworkConfigView, AppError> {
        let _guard = self.operations.lock().await;
        self.configuration_store.save(update)
    }

    pub async fn reset_configuration(self: &Arc<Self>) -> Result<NetworkConfigView, AppError> {
        let _guard = self.operations.lock().await;
        self.configuration_store.reset()
    }

    pub async fn versions(self: &Arc<Self>) -> Result<VersionInfo, AppError> {
        let this = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || {
            let bundled = verify_assets(&this.binaries).is_ok();
            let owned = platform::service_directory().ok().and_then(|root| {
                platform::owned_service_binary(&deployment::allowed_binaries(&root))
                    .ok()
                    .flatten()
            });
            let deployed = owned
                .as_ref()
                .and_then(|_| platform::service_description().ok().flatten())
                .and_then(|description| {
                    deployment::PublicDeployment::from_description(&description)
                });
            let running = if owned.is_some()
                && bundled
                && platform::service_state().ok() == Some(ServiceState::Running)
            {
                this.rpc::<status::NodeInfo>("node")
                    .ok()
                    .filter(|node| node.inst_id == network_config::INSTANCE_ID)
                    .and_then(|node| status::version(&node.version))
            } else {
                None
            };
            VersionInfo {
                app: env!("CARGO_PKG_VERSION").into(),
                easytier_target: EASYTIER_TARGET_VERSION.into(),
                easytier_bundled: bundled.then(|| EASYTIER_TARGET_VERSION.into()),
                easytier_deployed: deployed.as_ref().map(|value| value.core_version.clone()),
                easytier_running: running,
                engine_owner_app: deployed
                    .as_ref()
                    .map(|value| value.owner_app_version.to_string()),
                engine_revision: deployed.map(|value| value.engine_revision),
                protocol: rela_protocol::PROTOCOL_VERSION,
            }
        })
        .await
        .map_err(|_| internal_error())
    }

    fn status(&self) -> Result<ConnectionStatus, AppError> {
        let mut result = ConnectionStatus {
            last_error: None,
            ..ConnectionStatus::default()
        };
        if !FILES.iter().all(|name| self.binaries.join(name).is_file()) {
            result.last_error = Some("网络引擎文件不完整，请重新安装 Rela。".into());
            return Ok(result);
        }
        let state = platform::service_state()?;
        result.core = match state {
            ServiceState::NotInstalled | ServiceState::Stopped => CoreState::Stopped,
            ServiceState::Starting => CoreState::Starting,
            ServiceState::Stopping => CoreState::Stopping,
            ServiceState::Running => CoreState::Running,
        };
        if state != ServiceState::Running {
            return Ok(result);
        }
        platform::owned_service_binary(&deployment::allowed_binaries(
            &platform::service_directory()?,
        ))?;
        let node = match self.rpc::<status::NodeInfo>("node") {
            Ok(node) => node,
            Err(_) => {
                result.last_error = Some("网络引擎运行中，等待状态接口响应。".into());
                return Ok(result);
            }
        };
        if node.inst_id != network_config::INSTANCE_ID {
            result.last_error = Some("管理端口返回了其他网络实例，请检查端口占用。".into());
            return Ok(result);
        }
        let connectors = match self.rpc::<Vec<status::Connector>>("connector") {
            Ok(connectors) => connectors,
            Err(_) => {
                result.last_error = Some("暂时无法读取节点连接状态。".into());
                return Ok(result);
            }
        };
        let ip = status::parse_ipv4(&node.ipv4_addr);
        let tun_ready = ip.is_some_and(|ip| platform::tun_has_ip(network_config::TUN_NAME, ip));
        result = status::connection_snapshot(&node, &connectors, tun_ready);
        if result.connected {
            if let Some(gateway) = self
                .configuration_store
                .running_gateway()?
                .and_then(|ip| ip.parse().ok())
            {
                result.latency_ms = platform::ping(gateway);
                result.gateway = if result.latency_ms.is_some() {
                    rela_protocol::GatewayState::Online
                } else {
                    rela_protocol::GatewayState::Offline
                };
                if let Ok(routes) = self.rpc::<Vec<status::RouteRow>>("route") {
                    result.connection_type = status::gateway_connection_type(&routes, gateway);
                }
            }
        }
        Ok(result)
    }

    fn rpc<T: serde::de::DeserializeOwned>(&self, method: &str) -> Result<T, AppError> {
        rpc(&self.binaries, method)
    }
}

fn rpc<T: serde::de::DeserializeOwned>(directory: &Path, method: &str) -> Result<T, AppError> {
    let bytes = process::output(
        &directory.join("easytier-cli.exe"),
        &[
            "--rpc-portal".into(),
            RPC_PORTAL.into(),
            "--instance-name".into(),
            network_config::INSTANCE_NAME.into(),
            "--output".into(),
            "json".into(),
            method.into(),
        ],
        Duration::from_secs(3),
    )?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::new("core_response_invalid", "网络引擎返回了无法识别的状态。"))
}

fn internal_error() -> AppError {
    AppError::new("internal_error", "操作未完成，请重试。")
}
fn integrity_error() -> AppError {
    AppError::new(
        "core_integrity_failed",
        "网络引擎文件校验失败，请重新安装 Rela。",
    )
}

fn verify_assets(directory: &Path) -> Result<(), AppError> {
    asset_manifest()?.verify(directory)
}

fn asset_manifest() -> Result<AssetManifest, AppError> {
    let manifest: AssetManifest =
        serde_json::from_str(include_str!("../../../config/easytier-version.json"))
            .map_err(|_| integrity_error())?;
    manifest.validate()?;
    if manifest.version != EASYTIER_TARGET_VERSION || manifest.engine_revision == 0 {
        return Err(integrity_error());
    }
    Ok(manifest)
}

fn perform_helper(mut request: HelperRequest, bundled: &Path) -> Result<bool, AppError> {
    if !platform::is_elevated() {
        return Err(AppError::new("permission_required", "此操作需要系统授权。"));
    }
    rela_protocol::Preferences {
        device_name: request.device_name.clone(),
        ..Default::default()
    }
    .validate()?;
    let root = platform::service_directory()?;
    platform::secure_service_directory(&root)?;
    let _control_lock = platform::lock_service_control(&root)?;
    let mut service = managed_service::ManagedService::new(root.clone());
    let files = deployment::Files::new(
        root,
        platform::secure_service_directory,
        platform::secure_service_file,
    );
    if matches!(request.action, CoreAction::Disconnect) {
        use deployment::Service;
        // Disconnect never restarts a connection while repairing an interrupted update.
        service.stop()?;
        files.recover(&mut service, false)?;
        return Ok(false);
    }
    request
        .config
        .as_mut()
        .ok_or_else(internal_error)?
        .normalize_and_validate(true)?;
    verify_assets(bundled)?;
    files.recover(&mut service, true)?;
    if matches!(request.action, CoreAction::Connect)
        && platform::service_state()? == ServiceState::Running
    {
        return Ok(false);
    }
    let config = request
        .config
        .ok_or_else(internal_error)?
        .core_toml(&request.device_name)?;
    files.apply(bundled, config.as_bytes(), &mut service)?;
    Ok(true)
}

fn service_command(directory: &Path, args: &[OsString]) -> Result<(), AppError> {
    let mut full = vec!["service".into(), "--name".into(), SERVICE_NAME.into()];
    full.extend_from_slice(args);
    process::output(
        &directory.join("easytier-cli.exe"),
        &full,
        Duration::from_secs(25),
    )
    .map(|_| ())
}

fn wait_for(expected: ServiceState) -> Result<(), AppError> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if platform::service_state()? == expected {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(AppError::new(
                "service_timeout",
                "网络引擎服务未在预期时间内就绪。",
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

/// Used only by a transient elevated Rela instance; this does not start a GUI or daemon.
pub fn helper_entry() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_none_or(|arg| arg != "--rela-core-helper") {
        return None;
    }
    let result = (|| {
        if args.len() != 2 || !platform::is_elevated() {
            return Err(internal_error());
        }
        let request_path = PathBuf::from(&args[1]);
        let metadata = fs::symlink_metadata(&request_path).map_err(|_| internal_error())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
            return Err(internal_error());
        }
        let bytes = platform::unprotect(&fs::read(request_path).map_err(|_| internal_error())?)?;
        let request: HelperRequest =
            serde_json::from_slice(&bytes).map_err(|_| internal_error())?;
        let executable = std::env::current_exe().map_err(|_| internal_error())?;
        let bundled = executable
            .parent()
            .ok_or_else(internal_error)?
            .join("easytier");
        perform_helper(request, &bundled)
    })();
    Some(match result {
        Ok(true) => 0,
        Ok(false) => 10,
        Err(error) if error.code == "core_integrity_failed" => 2,
        Err(error) if error.code == "service_conflict" => 3,
        Err(error) if error.code == "core_downgrade_blocked" => 4,
        Err(error) if error.code == "core_recovery_required" => 5,
        Err(error) if error.code == "core_update_rolled_back" => 6,
        Err(error) if error.code == "credential_migration_required" => 7,
        Err(error) if error.code == "core_version_unknown" => 8,
        Err(_) => 1,
    })
}
