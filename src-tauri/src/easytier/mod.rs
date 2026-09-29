//! Rela 直接控制专用的 EasyTier Core Windows 服务，并通过官方 CLI 读取本机 RPC。
mod process;
mod status;

use crate::{
    network_config::{self, NetworkConfig, RPC_PORTAL},
    platform::{self, ServiceState, SERVICE_NAME},
};
use rela_protocol::{
    AppError, ConnectionStatus, CoreState, NetworkConfigUpdate, NetworkConfigView,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
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

#[derive(Deserialize)]
struct AssetManifest {
    version: String,
    files: BTreeMap<String, String>,
}

pub struct EasyTierCore {
    pub config_path: PathBuf,
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
    pub fn new(config_dir: PathBuf, resource_dir: PathBuf) -> Self {
        Self {
            config_path: config_dir.join("network.dat"),
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
            let config = if matches!(action, CoreAction::Disconnect) {
                None
            } else {
                let mut config = network_config::load(&this.config_path)?;
                config.normalize_and_validate(true)?;
                Some(config)
            };
            let request = HelperRequest {
                action,
                config,
                device_name,
            };
            if platform::is_elevated() {
                perform_helper(request, &this.binaries)?;
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
                platform::elevate_helper(&request_file.0)?;
            }
            this.status()
        })
        .await
        .map_err(|_| internal_error())?
    }

    pub async fn configuration(self: &Arc<Self>) -> Result<NetworkConfigView, AppError> {
        let _guard = self.operations.lock().await;
        Ok(network_config::load(&self.config_path)?.view())
    }

    pub async fn save_configuration(
        self: &Arc<Self>,
        update: NetworkConfigUpdate,
    ) -> Result<NetworkConfigView, AppError> {
        let _guard = self.operations.lock().await;
        let next = network_config::load(&self.config_path)?.updated(update)?;
        network_config::save(&self.config_path, &next)?;
        Ok(next.view())
    }

    pub async fn reset_configuration(self: &Arc<Self>) -> Result<NetworkConfigView, AppError> {
        let _guard = self.operations.lock().await;
        let defaults = NetworkConfig::bundled()?;
        network_config::save(&self.config_path, &defaults)?;
        Ok(defaults.view())
    }

    pub async fn installed_version(self: &Arc<Self>) -> Option<String> {
        let this = Arc::clone(self);
        tauri::async_runtime::spawn_blocking(move || {
            verify_assets(&this.binaries).ok()?;
            let bytes = process::output(
                &this.binaries.join("easytier-core.exe"),
                &["--version".into()],
                Duration::from_secs(3),
            )
            .ok()?;
            let text = String::from_utf8(bytes).ok()?;
            let version = text.trim().strip_prefix("easytier-core ")?;
            version
                .eq(EASYTIER_TARGET_VERSION)
                .then(|| version.to_owned())
        })
        .await
        .ok()
        .flatten()
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
        platform::verify_service_binary(
            &platform::service_directory()?.join("engine/easytier-core.exe"),
        )?;
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
            let config = network_config::load(&self.config_path)?;
            if let Some(gateway) = config.gateway_ip.and_then(|ip| ip.parse().ok()) {
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
        let bytes = process::output(
            &self.binaries.join("easytier-cli.exe"),
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
    let manifest: AssetManifest =
        serde_json::from_str(include_str!("../../../config/easytier-version.json"))
            .map_err(|_| integrity_error())?;
    if manifest.version != EASYTIER_TARGET_VERSION {
        return Err(integrity_error());
    }
    for name in FILES {
        let path = directory.join(name);
        if fs::symlink_metadata(&path)
            .map_err(|_| integrity_error())?
            .file_type()
            .is_symlink()
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
        if manifest.files.get(*name) != Some(&format!("{:x}", hash.finalize())) {
            return Err(integrity_error());
        }
    }
    Ok(())
}

fn perform_helper(mut request: HelperRequest, bundled: &Path) -> Result<(), AppError> {
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
    let engine = root.join("engine");
    let executable = engine.join("easytier-core.exe");
    platform::verify_service_binary(&executable)?;
    let state = platform::service_state()?;
    if matches!(request.action, CoreAction::Connect) && state == ServiceState::Running {
        return Ok(());
    }
    if !matches!(request.action, CoreAction::Disconnect) {
        request
            .config
            .as_mut()
            .ok_or_else(internal_error)?
            .normalize_and_validate(true)?;
        verify_assets(bundled)?;
    }
    if matches!(
        state,
        ServiceState::Running | ServiceState::Starting | ServiceState::Stopping
    ) {
        if state != ServiceState::Stopping {
            platform::stop_service()?;
        }
        wait_for(ServiceState::Stopped)?;
    }
    if matches!(request.action, CoreAction::Disconnect) {
        return Ok(());
    }
    let listener = TcpListener::bind(RPC_PORTAL)
        .map_err(|_| AppError::new("service_conflict", "网络管理端口已被其他程序占用。"))?;
    drop(listener);
    platform::secure_service_directory(&engine)?;
    for name in FILES {
        let target = engine.join(name);
        if target.exists()
            && fs::symlink_metadata(&target)
                .map_err(|_| integrity_error())?
                .file_type()
                .is_symlink()
        {
            return Err(integrity_error());
        }
        let bytes = fs::read(bundled.join(name)).map_err(|_| integrity_error())?;
        platform::atomic_write(&target, &bytes)?;
    }
    verify_assets(&engine)?;
    let config_path = root.join("core.toml");
    let config = request.config.ok_or_else(internal_error)?;
    platform::atomic_write(
        &config_path,
        config.core_toml(&request.device_name)?.as_bytes(),
    )?;
    let args: Vec<OsString> = vec![
        "install".into(),
        "--display-name".into(),
        "Rela Network".into(),
        "--description".into(),
        "Rela EasyTier network connection".into(),
        "--disable-autostart".into(),
        "true".into(),
        "--core-path".into(),
        executable.into_os_string(),
        "--service-work-dir".into(),
        engine.clone().into_os_string(),
        "--".into(),
        "--disable-env-parsing".into(),
        "--config-file".into(),
        config_path.into_os_string(),
        "--rpc-portal".into(),
        RPC_PORTAL.into(),
        "--rpc-portal-whitelist".into(),
        "127.0.0.1/32".into(),
        "--no-listener".into(),
    ];
    if state == ServiceState::NotInstalled {
        service_command(&engine, &args)?;
    }
    platform::start_service()?;
    wait_for(ServiceState::Running)
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
        Ok(()) => 0,
        Err(error) if error.code == "core_integrity_failed" => 2,
        Err(error) if error.code == "service_conflict" => 3,
        Err(_) => 1,
    })
}
