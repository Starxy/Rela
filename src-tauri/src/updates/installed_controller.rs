//! Ordinary-user owner of an installed update. The official updater runs in a
//! separate launcher because its successful Windows install exits its caller.
//! Machine decisions belong to the authenticated, protected NSIS participant.
mod launcher;
mod runtime;
mod worker;

use super::{
    controller::{copy_runner, read_private, write_private},
    installed::{self, guard},
    installer::InstallerPlan,
    ipc::{self, Channel, Endpoint, Listener, Role},
    nsis_worker::{self, Request, VerifiedSource},
    portable::{exists, fingerprint, MAX_PROGRAM},
    process::{ProcessFence, ProcessTicket},
    startup::{InstanceLease, StartupRequest},
};
use crate::{
    distribution::{
        package::{self, VerifiedPackage},
        portable,
        software::SelectedUpdate,
        transport::Fetcher,
        DistributionConfig,
    },
    platform,
};
use rela_manifests::sha256;
use rela_protocol::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    os::windows::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

const RECORD: &str = "control.dat";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Waiting,
    ConfigPreparing,
    ConfigPrepared,
    Launching,
    Active,
    Committed,
    RolledBack,
}
impl Phase {
    fn terminal(self) -> bool {
        matches!(self, Self::Committed | Self::RolledBack)
    }
    fn machine_may_have_started(self) -> bool {
        matches!(self, Self::Active | Self::Committed | Self::RolledBack)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    schema_version: u32,
    request: Request,
    config: PathBuf,
    parent: ProcessTicket,
    handoff: Endpoint,
    runner: Option<ProcessTicket>,
    launcher: Option<ProcessTicket>,
    launcher_endpoint: Option<Endpoint>,
    hook: Option<ProcessTicket>,
    installer: Option<ProcessTicket>,
    installer_image: Option<PathBuf>,
    candidate: Option<ProcessTicket>,
    phase: Phase,
    machine_done: bool,
    failure: Option<String>,
}
impl Control {
    fn validate(&self) -> Result<(), AppError> {
        self.request.validate()?;
        self.handoff.validate()?;
        if self.schema_version != 1
            || !self.config.is_absolute()
            || self.handoff.id != self.request.id
            || self.handoff.role != Role::Controller
            || (self.machine_done && !self.phase.terminal())
            || self.installer.is_some() != self.installer_image.is_some()
            || self
                .installer_image
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
            || self.failure.as_ref().is_some_and(|text| text.len() > 8192)
        {
            return Err(error());
        }
        for ticket in std::iter::once(&self.parent)
            .chain(self.runner.iter())
            .chain(self.launcher.iter())
            .chain(self.hook.iter())
            .chain(self.installer.iter())
            .chain(self.candidate.iter())
        {
            validate_ticket(ticket)?;
        }
        if let Some(endpoint) = &self.launcher_endpoint {
            endpoint.validate()?;
            if endpoint.id != self.request.id || endpoint.role != Role::InstallerLauncher {
                return Err(error());
            }
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pointer {
    schema_version: u32,
    id: String,
    root: PathBuf,
    config: PathBuf,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Handoff {
    Ready,
}

fn validate_ticket(ticket: &ProcessTicket) -> Result<(), AppError> {
    if ticket.pid == 0
        || ticket
            .started_at
            .parse::<u64>()
            .ok()
            .filter(|v| *v > 0)
            .is_none()
    {
        return Err(error());
    }
    Ok(())
}
fn config_location(config: &Path) -> Result<PathBuf, AppError> {
    let parent = config.parent().ok_or_else(error)?;
    package::plain_directory(parent)?;
    Ok(parent
        .canonicalize()
        .map_err(|_| error())?
        .join(config.file_name().ok_or_else(error)?))
}
fn pointer_location(config: &Path) -> Result<PathBuf, AppError> {
    let config = config_location(config)?;
    let digest = sha256(config.to_string_lossy().to_lowercase().as_bytes());
    Ok(config
        .parent()
        .ok_or_else(error)?
        .join(format!(".rela-installed-active-{digest}.dat")))
}
fn control_location(config: &Path, id: &str) -> Result<PathBuf, AppError> {
    ipc::validate_id(id)?;
    Ok(config_location(config)?
        .parent()
        .ok_or_else(error)?
        .join(format!(".rela-installed-user-{id}")))
}
fn archive_location(control: &Path) -> Result<PathBuf, AppError> {
    Ok(control.with_file_name(format!(
        "{}-finished",
        control.file_name().ok_or_else(error)?.to_string_lossy()
    )))
}
fn save(control: &Path, state: &Control) -> Result<(), AppError> {
    state.validate()?;
    write_private(&control.join(RECORD), state)
}
fn load(control: &Path) -> Result<Control, AppError> {
    package::plain_directory(control)?;
    let state: Control = read_private(&control.join(RECORD))?;
    state.validate()?;
    let expected = control_location(&state.config, &state.request.id)?;
    let actual = control.canonicalize().map_err(|_| error())?;
    if actual != expected
        && (actual != archive_location(&expected)?
            || !state.phase.terminal()
            || !state.machine_done)
    {
        return Err(error());
    }
    Ok(state)
}
fn pointed(config: &Path, root: &Path) -> Result<Option<(PathBuf, Control)>, AppError> {
    let path = pointer_location(config)?;
    if !exists(&path)? {
        return Ok(None);
    }
    let pointer: Pointer = read_private(&path)?;
    if pointer.schema_version != 1
        || pointer.config != config_location(config)?
        || pointer.root != root.canonicalize().map_err(|_| error())?
    {
        return Err(error());
    }
    let mut control = control_location(config, &pointer.id)?;
    if !exists(&control)? {
        control = archive_location(&control)?;
    }
    let state = load(&control)?;
    if state.request.id != pointer.id
        || state.request.root != pointer.root
        || state.config != pointer.config
    {
        return Err(error());
    }
    Ok(Some((control, state)))
}
fn publish_pointer(control: &Path, state: &Control) -> Result<(), AppError> {
    let path = pointer_location(&state.config)?;
    if exists(&path)? || control != control_location(&state.config, &state.request.id)? {
        return Err(error());
    }
    write_private(
        &path,
        &Pointer {
            schema_version: 1,
            id: state.request.id.clone(),
            root: state.request.root.clone(),
            config: state.config.clone(),
        },
    )
}
fn lock_file(path: &Path, digest: &str) -> Result<File, AppError> {
    package::plain_file(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(path)
        .map_err(|_| error())?;
    if fingerprint(path, MAX_PROGRAM)?.sha256 != digest {
        return Err(error());
    }
    Ok(file)
}
fn verify(
    control: &Path,
    state: &Control,
) -> Result<(VerifiedSource, VerifiedPackage, File), AppError> {
    verify_using(
        control,
        state,
        &DistributionConfig::bundled()?,
        &super::package_public_key()?,
    )
}
fn verify_using(
    control: &Path,
    state: &Control,
    distribution: &DistributionConfig,
    key: &str,
) -> Result<(VerifiedSource, VerifiedPackage, File), AppError> {
    state.validate()?;
    let runner = lock_file(&control.join("runner.exe"), &state.request.runner_digest)?;
    let source = nsis_worker::verify_source_using(control, &state.request, distribution, key)?;
    let installer = VerifiedPackage::open_versioned(
        &control.join("installer.exe"),
        &source.manifest.installer,
        key,
        &source.manifest.version,
    )?;
    Ok((source, installer, runner))
}
fn spawn(executable: &Path, mode: &str, id: &str) -> Result<Child, AppError> {
    Command::new(executable)
        .args([mode, id])
        .current_dir(executable.parent().ok_or_else(error)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| error())
}

/// Called only after native selection of the exact version explicitly confirmed
/// by the user. The GUI remains alive until an authenticated helper takes over.
pub async fn start(
    selected: SelectedUpdate,
    config: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), AppError> {
    if platform::is_elevated() {
        return Err(AppError::new(
            "update_elevated_gui",
            "请以普通用户方式启动 Rela 后更新，安装程序会单独请求管理员授权。",
        ));
    }
    let executable = std::env::current_exe().map_err(|_| error())?;
    if executable
        .file_name()
        .is_none_or(|name| !name.to_string_lossy().eq_ignore_ascii_case("Rela.exe"))
    {
        return Err(error());
    }
    let root = executable
        .parent()
        .ok_or_else(error)?
        .canonicalize()
        .map_err(|_| error())?;
    if exists(&root.join("portable.txt"))? || exists(&pointer_location(config)?)? {
        return Err(error());
    }
    let _installation = guard::Lease::shared(&root)?;
    let config = config_location(config)?;
    let current = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| error())?;
    let distribution = DistributionConfig::bundled()?;
    let plan = InstallerPlan::from_signed(
        &selected.envelope,
        &distribution.keys,
        selected.manifest.channel,
        &current,
        &selected.manifest.version,
    )?;
    let id = ipc::random_id()?;
    let control = control_location(&config, &id)?;
    platform::create_private_directory(&control)?;
    let key = super::package_public_key()?;
    let manifest = plan.manifest();
    let total = manifest
        .installer
        .size
        .checked_add(manifest.portable.size)
        .ok_or_else(error)?;
    let mut download = plan
        .download(&key, &control, |n, _| progress(n, total))
        .await?;
    let installer =
        download.persist_copy(&control.join("installer.exe"), &manifest.installer, &key)?;
    let mut zip_download = Fetcher::for_packages()?
        .download_package(&manifest.portable, &key, &control, |n, _| {
            progress(manifest.installer.size + n, total)
        })
        .await?;
    let mut zip =
        zip_download.persist_copy(&control.join("update.zip"), &manifest.portable, &key)?;
    let stage = portable::stage(&mut zip, manifest, &control.join("candidate"))?;
    stage.verify()?;
    platform::atomic_write(&control.join("software.json"), &selected.envelope)?;
    copy_runner(&executable, &control.join("runner.exe"))?;
    // Relative to this trusted private directory; no user path is interpolated
    // into cmd syntax, so spaces/metacharacters in AppData cannot become code.
    platform::atomic_write(
        &control.join("Recover-Rela.cmd"),
        format!("@echo off\r\n\"%~dp0runner.exe\" --rela-installed-rescue {id}\r\n").as_bytes(),
    )?;
    let listener = Listener::bind(&id, Role::Controller)?;
    let pre = Listener::bind(&id, Role::InstallerPre)?;
    let post = Listener::bind(&id, Role::InstallerPost)?;
    let parent = ProcessFence::capture(std::process::id(), &executable)?;
    let state = Control {
        schema_version: 1,
        request: Request {
            schema_version: 1,
            id: id.clone(),
            root,
            previous_version: current,
            channel: manifest.channel,
            envelope_digest: sha256(&selected.envelope),
            runner_digest: fingerprint(&control.join("runner.exe"), MAX_PROGRAM)?.sha256,
            controller: parent.clone(),
            pre: pre.endpoint(),
            post: post.endpoint(),
            recovery: None,
        },
        config,
        parent,
        handoff: listener.endpoint(),
        runner: None,
        launcher: None,
        launcher_endpoint: None,
        hook: None,
        installer: None,
        installer_image: None,
        candidate: None,
        phase: Phase::Waiting,
        machine_done: false,
        failure: None,
    };
    save(&control, &state)?;
    publish_pointer(&control, &state)?;
    drop((zip, zip_download, download, installer));
    tauri::async_runtime::spawn_blocking(move || hand_off(&control, listener, false))
        .await
        .map_err(|_| error())?
}

fn hand_off(control: &Path, listener: Listener, recovering: bool) -> Result<(), AppError> {
    let state = load(control)?;
    let runner = control.join("runner.exe");
    let _guard = lock_file(&runner, &state.request.runner_digest)?;
    let mut child = spawn(
        &runner,
        if recovering {
            "--rela-installed-controller-recover"
        } else {
            "--rela-installed-update"
        },
        &state.request.id,
    )?;
    let result = (|| {
        let mut channel = listener.accept(child.id(), &runner, Duration::from_secs(60), || {
            child.try_wait().is_ok_and(|s| s.is_some())
        })?;
        match channel.receive::<Handoff>()? {
            Handoff::Ready => Ok(()),
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub fn entry() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mode = args.first().and_then(|arg| arg.to_str());
    if !matches!(
        mode,
        Some(
            "--rela-installed-update"
                | "--rela-installed-controller-recover"
                | "--rela-installed-launch"
                | "--rela-installed-rescue"
        )
    ) {
        return None;
    }
    let result = (|| {
        if args.len() != 2 || platform::is_elevated() {
            return Err(error());
        }
        let id = args[1].to_str().ok_or_else(error)?;
        ipc::validate_id(id)?;
        let executable = std::env::current_exe().map_err(|_| error())?;
        let control = executable
            .parent()
            .ok_or_else(error)?
            .canonicalize()
            .map_err(|_| error())?;
        let state = load(&control)?;
        if state.request.id != id
            || executable.file_name().is_none_or(|n| n != "runner.exe")
            || control != control_location(&state.config, id)?
            || state.request.previous_version
                != Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| error())?
        {
            return Err(error());
        }
        if mode == Some("--rela-installed-launch") {
            launcher::run(&control)
        } else {
            runtime::run(
                &control,
                mode != Some("--rela-installed-update"),
                mode == Some("--rela-installed-rescue"),
            )
        }
    })();
    // A launcher failure is reported through IPC; only the owning controller
    // presents recovery information after its bounded attempt has ended.
    if mode != Some("--rela-installed-launch") {
        if let Err(failure) = &result {
            platform::show_update_error(&failure.message);
        }
    }
    Some(if result.is_ok() { 0 } else { 1 })
}

pub(crate) fn resume_before_gui(root: &Path, config: &Path) -> Result<bool, AppError> {
    let Some((control, mut state)) = pointed(config, root)? else {
        return Ok(false);
    };
    if state
        .runner
        .as_ref()
        .map(ProcessFence::probe)
        .transpose()?
        .flatten()
        .is_some()
    {
        return Err(error());
    }
    if state.phase.terminal() && state.machine_done {
        archive_terminal(root, config, &state.request.id, state.phase)?;
        return Ok(false);
    }
    let (_source, _installer, _runner) = verify(&control, &state)?;
    let listener = Listener::bind(&state.request.id, Role::Controller)?;
    state.parent = ProcessFence::capture(
        std::process::id(),
        &std::env::current_exe().map_err(|_| error())?,
    )?;
    state.handoff = listener.endpoint();
    save(&control, &state)?;
    hand_off(&control, listener, true)?;
    Ok(true)
}

pub(crate) struct Candidate {
    pub channel: Channel,
    pub controller: ProcessFence,
}
pub(crate) fn candidate(root: &Path, config: &Path, id: &str) -> Result<Candidate, AppError> {
    let (control, state) = pointed(config, root)?.ok_or_else(error)?;
    let request: StartupRequest = read_private(&control.join("startup.dat"))?;
    request.endpoint.validate()?;
    let runner = state.runner.as_ref().ok_or_else(error)?;
    if state.request.id != id
        || !matches!(state.phase, Phase::Active | Phase::Committed)
        || request.endpoint.id != id
        || request.endpoint.role != Role::Candidate
        || &request.controller != runner
        || request.expected_version
            != Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| error())?
    {
        return Err(error());
    }
    let controller = ProcessFence::open(runner, &control.join("runner.exe"))?;
    if controller.has_exited()? {
        return Err(error());
    }
    // The listener accepts only after persisting the Child handle's identity.
    // Connecting first removes the process-spawn/read-record startup race.
    let channel = Channel::connect(&request.endpoint)?;
    let state = load(&control)?;
    let myself = ProcessFence::capture(std::process::id(), &root.join("Rela.exe"))?;
    if state.candidate.as_ref() != Some(&myself)
        || state.runner.as_ref() != Some(&request.controller)
    {
        return Err(error());
    }
    let (source, _installer, _guard) = verify(&control, &state)?;
    if source.manifest.version != request.expected_version {
        return Err(error());
    }
    let _live = lock_program(root, source.stage.index())?;
    Ok(Candidate {
        channel,
        controller,
    })
}

pub(crate) fn restored(root: &Path, config: &Path, id: &str) -> Result<Option<String>, AppError> {
    let (_, state) = pointed(config, root)?.ok_or_else(error)?;
    if state.request.id != id || state.phase != Phase::RolledBack || !state.machine_done {
        return Err(error());
    }
    if let Some(ticket) = &state.runner {
        if let Some(process) = ProcessFence::probe(ticket)? {
            process.wait(Duration::from_secs(20))?;
        }
    }
    let notice = state
        .failure
        .clone()
        .or_else(|| Some("上次更新未完成，已恢复原程序和配置。".into()));
    archive_terminal(root, config, id, Phase::RolledBack)?;
    Ok(notice)
}
pub(crate) fn finish_candidate(root: &Path, config: &Path, id: &str) -> Result<(), AppError> {
    archive_terminal(root, config, id, Phase::Committed)
}
fn archive_terminal(root: &Path, config: &Path, id: &str, expected: Phase) -> Result<(), AppError> {
    let Some((control, state)) = pointed(config, root)? else {
        return Ok(());
    };
    if state.request.id != id
        || state.phase != expected
        || !state.machine_done
        || !expected.terminal()
        || state
            .runner
            .as_ref()
            .map(ProcessFence::probe)
            .transpose()?
            .flatten()
            .is_some()
        || guard::pending_update(root)?
    {
        return Err(error());
    }
    let (source, installer, runner) = verify(&control, &state)?;
    if expected == Phase::Committed {
        let _guard = lock_program(root, source.stage.index())?;
    } else if fingerprint(&root.join("Rela.exe"), MAX_PROGRAM)?.sha256
        != state.request.runner_digest
    {
        return Err(error());
    }
    drop((source, installer, runner));
    let active = control_location(config, id)?;
    if control == active {
        platform::move_file_new(&control, &archive_location(&active)?)?;
    }
    fs::remove_file(pointer_location(config)?).map_err(|_| error())
}

/// Installed layout omits Portable's README/checksums/marker. It still requires
/// every common payload byte authenticated by the signed companion ZIP.
struct ProgramGuard {
    _files: Vec<File>,
}
fn lock_program(root: &Path, index: &portable::PortableIndex) -> Result<ProgramGuard, AppError> {
    let mut files = Vec::new();
    for name in installed::program_names() {
        files.push(lock_file(
            &root.join(name),
            index.files.get(name).ok_or_else(error)?,
        )?);
    }
    Ok(ProgramGuard { _files: files })
}
fn error() -> AppError {
    nsis_worker::error()
}

#[cfg(test)]
mod tests;
