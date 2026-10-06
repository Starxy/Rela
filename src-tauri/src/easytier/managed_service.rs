use super::{
    deployment::{self, Location, PublicDeployment, Service, ServiceSnapshot},
    *,
};
use std::net::TcpListener;

pub(super) struct ManagedService {
    root: PathBuf,
    rpc_binaries: PathBuf,
}
impl ManagedService {
    pub fn new(root: PathBuf, rpc_binaries: PathBuf) -> Self {
        Self { root, rpc_binaries }
    }
    fn location(&self) -> Result<Option<Location>, AppError> {
        let owned = platform::owned_service_binary(&deployment::allowed_binaries(&self.root))?;
        Ok(owned.map(|path| {
            if path == Location::Managed.executable(&self.root) {
                Location::Managed
            } else {
                Location::Legacy
            }
        }))
    }
}

impl Service for ManagedService {
    fn snapshot(&mut self) -> Result<ServiceSnapshot, AppError> {
        Ok(ServiceSnapshot {
            location: self.location()?,
            was_running: matches!(
                platform::service_state()?,
                ServiceState::Running | ServiceState::Starting
            ),
            description: platform::service_description()?,
            security: platform::service_security()?,
        })
    }
    fn begin_change(&mut self) -> Result<(), AppError> {
        if self.location()?.is_some() {
            platform::authorize_service_controller(None)?;
            platform::set_service_description("Rela network deployment pending")?;
        }
        Ok(())
    }
    fn stop(&mut self) -> Result<(), AppError> {
        self.location()?;
        match platform::service_state()? {
            ServiceState::NotInstalled | ServiceState::Stopped => return Ok(()),
            ServiceState::Stopping => {}
            _ => platform::stop_service()?,
        }
        wait_for(ServiceState::Stopped)
    }
    fn register(&mut self, location: Location) -> Result<(), AppError> {
        self.location()?;
        verify_assets(&self.rpc_binaries)?;
        // The official CLI updates both SCM and EasyTier's per-service working directory.
        // Use the verified bundled CLI even when restoring a historical engine:
        // older CLI versions parse the core-argument delimiter differently.
        service_command(&self.rpc_binaries, &install_args(&self.root, location))
            .map_err(|_| AppError::new("service_registration_failed", "网络服务注册失败。"))?;
        if self.location()? != Some(location) {
            return Err(integrity_error());
        }
        Ok(())
    }
    fn start_and_verify(&mut self, version: &str) -> Result<(), AppError> {
        self.location()?.ok_or_else(internal_error)?;
        let listener = TcpListener::bind(RPC_PORTAL)
            .map_err(|_| AppError::new("service_conflict", "网络管理端口已被其他程序占用。"))?;
        drop(listener);
        platform::start_service()?;
        wait_for(ServiceState::Running)?;
        self.verify_running(version)
    }
    fn verify_running(&mut self, version: &str) -> Result<(), AppError> {
        let location = self.location()?.ok_or_else(internal_error)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if self.location()? != Some(location)
                || platform::service_state()? != ServiceState::Running
            {
                return Err(AppError::new(
                    "core_start_failed",
                    "网络引擎启动后退出，请检查网络配置与驱动。",
                ));
            }
            if let Ok(node) = rpc::<status::NodeInfo>(&self.rpc_binaries, "node") {
                if node.inst_id != network_config::INSTANCE_ID {
                    return Err(AppError::new(
                        "service_conflict",
                        "管理端口返回了其他网络实例。",
                    ));
                }
                if status::version(&node.version).as_deref() == Some(version) {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err(AppError::new(
                    "core_start_failed",
                    "网络引擎启动后未通过实例和版本校验。",
                ));
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
    fn restore_registration(&mut self, previous: &ServiceSnapshot) -> Result<(), AppError> {
        self.location()?;
        match previous.location {
            Some(location) => {
                self.register(location)?;
                platform::set_service_description(previous.description.as_deref().unwrap_or(""))?;
                if let Some(security) = &previous.security {
                    platform::set_service_security(security)?;
                }
                Ok(())
            }
            None => platform::delete_service(),
        }
    }
    fn publish(&mut self, deployment: &PublicDeployment) -> Result<(), AppError> {
        if self.location()? != Some(Location::Managed) {
            return Err(integrity_error());
        }
        platform::set_service_description(&deployment.description()?)
    }
}

fn install_args(root: &Path, location: Location) -> Vec<OsString> {
    vec![
        "install".into(),
        "--display-name".into(),
        "Rela Network".into(),
        "--description".into(),
        "Rela EasyTier network connection".into(),
        "--disable-autostart".into(),
        "true".into(),
        "--disable-restart-on-failure".into(),
        "true".into(),
        "--core-path".into(),
        location.executable(root).into_os_string(),
        "--service-work-dir".into(),
        location.directory(root).into_os_string(),
        "--core-args".into(),
        "--disable-env-parsing".into(),
        "--config-file".into(),
        root.join("core.toml").into_os_string(),
        "--rpc-portal".into(),
        RPC_PORTAL.into(),
        "--rpc-portal-whitelist".into(),
        "127.0.0.1/32".into(),
        "--no-listener".into(),
    ]
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;

    #[test]
    fn pinned_cli_accepts_service_arguments_without_mutating_scm() {
        let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries/easytier");
        verify_assets(&bundled).unwrap();
        let missing = std::env::temp_dir().join(format!(
            "rela-cli-parse-only-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(!missing.exists());
        let args = install_args(&missing, Location::Managed);
        let probe = |args: &[OsString]| {
            std::process::Command::new(bundled.join("easytier-cli.exe"))
                .args(["service", "--name", SERVICE_NAME])
                .args(args)
                .creation_flags(0x08000000)
                .output()
                .unwrap()
        };
        // An elevated process fails on the missing executable before installing;
        // an ordinary process can fail earlier when opening SCM. Neither mutates it.
        let corrected = probe(&args);
        assert_eq!(corrected.status.code(), Some(1));
        let error = String::from_utf8_lossy(&corrected.stderr);
        assert!(
            error.contains("failed to get easytier core application")
                || error.contains("os error 5")
        );
        let mut old = args;
        *old.iter_mut().find(|arg| *arg == "--core-args").unwrap() = "--".into();
        let rejected = probe(&old);
        assert_eq!(rejected.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&rejected.stderr).contains("unexpected argument"));
    }
}
