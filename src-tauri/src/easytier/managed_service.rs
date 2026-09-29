use super::{
    deployment::{self, Location, PublicDeployment, Service, ServiceSnapshot},
    *,
};
use std::net::TcpListener;

pub(super) struct ManagedService {
    root: PathBuf,
}
impl ManagedService {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
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
        })
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
        let engine = location.directory(&self.root);
        // The official CLI updates both SCM and EasyTier's per-service working directory.
        // Passing only fixed flags also preserves manual startup and the loopback RPC policy.
        service_command(
            &engine,
            &[
                "install".into(),
                "--display-name".into(),
                "Rela Network".into(),
                "--description".into(),
                "Rela EasyTier network connection".into(),
                "--disable-autostart".into(),
                "true".into(),
                "--core-path".into(),
                location.executable(&self.root).into_os_string(),
                "--service-work-dir".into(),
                engine.clone().into_os_string(),
                "--".into(),
                "--disable-env-parsing".into(),
                "--config-file".into(),
                self.root.join("core.toml").into_os_string(),
                "--rpc-portal".into(),
                RPC_PORTAL.into(),
                "--rpc-portal-whitelist".into(),
                "127.0.0.1/32".into(),
                "--no-listener".into(),
            ],
        )?;
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
                return Err(integrity_error());
            }
            if let Ok(node) = rpc::<status::NodeInfo>(&location.directory(&self.root), "node") {
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
                platform::set_service_description(previous.description.as_deref().unwrap_or(""))
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
