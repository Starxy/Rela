//! Candidate readiness and one writer per configuration directory.
use super::{
    controller,
    gate::Gate,
    installed::guard as installed_guard,
    installed_controller,
    ipc::{Channel, Endpoint, Role},
    process::{ProcessFence, ProcessTicket},
};
use crate::distribution::package;
use rela_protocol::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

pub struct InstanceLease {
    _file: File,
    key: String,
}
impl InstanceLease {
    pub fn acquire(config: &Path) -> Result<Self, AppError> {
        // Never place the lease in the configuration tree which rollback renames.
        let parent = config.parent().ok_or_else(controller::error)?;
        for ancestor in parent.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            match fs::symlink_metadata(ancestor) {
                Ok(_) => package::plain_directory(ancestor)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(controller::error()),
            }
        }
        fs::create_dir_all(parent).map_err(|_| controller::error())?;
        package::plain_directory(parent)?;
        if config.try_exists().map_err(|_| controller::error())? {
            package::plain_directory(config)?;
        }
        let normalized = parent
            .canonicalize()
            .map_err(|_| controller::error())?
            .join(config.file_name().ok_or_else(controller::error)?)
            .to_string_lossy()
            .to_lowercase();
        let path = parent.join(format!(
            ".rela-instance-{}.lock",
            rela_manifests::sha256(normalized.as_bytes())
        ));
        if path.try_exists().map_err(|_| controller::error())? {
            package::plain_file(&path)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(3)
            .open(path)
            .map_err(|_| controller::error())?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
            AppError::new(
                "instance_running",
                "这份 Rela 已在运行或正在更新，请先退出已有窗口。",
            )
        })?;
        Ok(Self {
            _file: file,
            key: normalized,
        })
    }

    pub(super) fn protects(&self, config: &Path) -> Result<(), AppError> {
        let parent = config.parent().ok_or_else(controller::error)?;
        package::plain_directory(parent)?;
        let normalized = parent
            .canonicalize()
            .map_err(|_| controller::error())?
            .join(config.file_name().ok_or_else(controller::error)?)
            .to_string_lossy()
            .to_lowercase();
        if normalized != self.key {
            return Err(controller::error());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupRequest {
    pub endpoint: Endpoint,
    pub controller: ProcessTicket,
    pub expected_version: Version,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupReady {
    pub version: Version,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupDecision {
    Release,
    Abort,
}

struct Pending {
    channel: Channel,
    controller: ProcessFence,
    root: PathBuf,
    id: String,
    installed_config: Option<PathBuf>,
}
pub struct Startup {
    pub gate: Arc<Gate>,
    pub notice: Option<String>,
    _lease: InstanceLease,
    _installation: Option<installed_guard::Lease>,
    pending: Mutex<Option<Pending>>,
}
impl Startup {
    /// Called before constructing config stores, the WebView or background jobs.
    pub fn initialize(config: &Path) -> Result<Self, AppError> {
        let lease = InstanceLease::acquire(config)?;
        let executable = std::env::current_exe().map_err(|_| controller::error())?;
        let root = executable.parent().ok_or_else(controller::error)?;
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let mode = args.first().and_then(|v| v.to_str());
        let reserved = mode.is_some_and(|v| v.starts_with("--rela-"));
        if reserved
            && (!matches!(
                mode,
                Some(
                    "--rela-updated"
                        | "--rela-restored"
                        | "--rela-installed-updated"
                        | "--rela-installed-restored"
                )
            ) || args.len() != 2)
        {
            if matches!(
                mode,
                Some("--rela-installed-updated" | "--rela-installed-restored")
            ) {
                return Err(controller::error());
            }
            return Err(controller::error());
        }
        let mut pending = None;
        let mut notice = None;
        let mut installation = None;
        if root
            .join("portable.txt")
            .try_exists()
            .map_err(|_| controller::error())?
        {
            package::plain_file(&root.join("portable.txt"))?;
            if let Ok(state) = controller::load(root) {
                if state.body == controller::BodyState::RolledBack {
                    notice = Some(
                        state
                            .failure
                            .unwrap_or_else(|| "上次更新未完成，已恢复原程序和配置。".into()),
                    );
                }
            }
            if mode == Some("--rela-updated") {
                let id = args[1].to_str().ok_or_else(controller::error)?;
                let request: StartupRequest = controller::read_private(
                    &root.join(controller::DIRECTORY).join("startup.dat"),
                )?;
                let state = controller::load(root)?;
                let runner = state.runner.ok_or_else(controller::error)?;
                request.endpoint.validate()?;
                if request.endpoint.id != id
                    || state.id != id
                    || request.endpoint.role != Role::Candidate
                    || request.expected_version
                        != Version::parse(env!("CARGO_PKG_VERSION"))
                            .map_err(|_| controller::error())?
                    || request.controller.pid != runner.pid
                    || request.controller.started_at != runner.started_at
                {
                    return Err(controller::error());
                }
                let controller = ProcessFence::open(
                    &request.controller,
                    &root.join(controller::DIRECTORY).join("runner.exe"),
                )?;
                if controller.has_exited()? {
                    return Err(controller::error());
                }
                pending = Some(Pending {
                    channel: Channel::connect(&request.endpoint)?,
                    controller,
                    root: root.into(),
                    id: id.into(),
                    installed_config: None,
                });
            } else if mode == Some("--rela-restored") {
                controller::restored(root, args[1].to_str().ok_or_else(controller::error)?)?;
            } else if controller::resume_before_gui(root)? {
                // The authenticated runner cannot mutate live files until this exit.
                std::process::exit(0);
            }
        } else {
            if matches!(mode, Some("--rela-updated" | "--rela-restored")) {
                return Err(controller::error());
            }
            if mode == Some("--rela-installed-updated") {
                let id = args[1].to_str().ok_or_else(controller::error)?;
                let candidate = installed_controller::candidate(root, config, id)?;
                installation = Some(installed_guard::Lease::candidate(root)?);
                pending = Some(Pending {
                    channel: candidate.channel,
                    controller: candidate.controller,
                    root: root.into(),
                    id: id.into(),
                    installed_config: Some(config.into()),
                });
            } else if mode == Some("--rela-installed-restored") {
                notice = installed_controller::restored(
                    root,
                    config,
                    args[1].to_str().ok_or_else(controller::error)?,
                )?;
            } else if installed_controller::resume_before_gui(root, config)? {
                std::process::exit(0);
            }
            // First manual installation creates this protected read-only lock.
            // Development/legacy layouts without it remain usable, but cannot
            // pass the installed updater's exclusive-lease preflight.
            if pending.is_none() && installed_guard::pending_update(root)? {
                return Err(super::nsis_worker::error());
            }
            if installation.is_none()
                && root
                    .join(installed_guard::RUNNING_LOCK)
                    .try_exists()
                    .map_err(|_| controller::error())?
            {
                installation = Some(installed_guard::Lease::shared(root)?);
            }
        }
        Ok(Self {
            gate: Arc::new(Gate::new(pending.is_some())),
            notice,
            _lease: lease,
            _installation: installation,
            pending: Mutex::new(pending),
        })
    }

    pub fn complete(&self) -> Result<(), AppError> {
        let pending = self.pending.lock().map_err(|_| controller::error())?.take();
        let Some(mut pending) = pending else {
            return Ok(());
        };
        if pending.controller.has_exited()? {
            return Err(controller::error());
        }
        pending.channel.timeout(Duration::from_secs(180))?;
        pending.channel.send(&StartupReady {
            version: Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| controller::error())?,
        })?;
        match pending.channel.receive()? {
            StartupDecision::Abort => Err(controller::error()),
            StartupDecision::Release => {
                // A release is sent only after both durable commits and body cleanup.
                pending.controller.wait(Duration::from_secs(15))?;
                if let Some(config) = &pending.installed_config {
                    installed_controller::finish_candidate(&pending.root, config, &pending.id)?;
                } else {
                    controller::archive_terminal(
                        &pending.root,
                        &pending.id,
                        controller::BodyState::Committed,
                    )?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lease_survives_config_rename_and_releases_on_drop() {
        let root = std::env::temp_dir().join(format!(
            "rela-instance-{}",
            super::super::ipc::random_id().unwrap()
        ));
        let config = root.join("data/config");
        fs::create_dir_all(&config).unwrap();
        let lease = InstanceLease::acquire(&config).unwrap();
        fs::rename(&config, root.join("data/config-backup")).unwrap();
        fs::create_dir(&config).unwrap();
        assert!(InstanceLease::acquire(&config).is_err());
        drop(lease);
        drop(InstanceLease::acquire(&config).unwrap());
        fs::remove_dir(&config).unwrap();
        fs::remove_dir(root.join("data/config-backup")).unwrap();
        for file in fs::read_dir(root.join("data")).unwrap() {
            fs::remove_file(file.unwrap().path()).unwrap();
        }
        fs::remove_dir(root.join("data")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
