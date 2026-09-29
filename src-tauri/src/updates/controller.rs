//! Non-elevated Portable update controller. Targets are inferred from its fixed
//! location; every restart re-verifies the signed feed, ZIP and candidate tree.
use super::{
    coordination::Permit,
    core_worker,
    ipc::{self, Channel, Endpoint, Listener, Role},
    portable::{Phase, Runtime, Transaction},
    process::{ProcessFence, ProcessTicket},
    startup::{InstanceLease, StartupDecision, StartupReady, StartupRequest},
};
use crate::{
    distribution::{
        self,
        package::{self, VerifiedPackage},
        portable::{self, PortableStage},
        software::SelectedUpdate,
        transport::Fetcher,
        DistributionConfig,
    },
    platform,
};
use rela_manifests::{
    sha256, verify_envelope, Channel as UpdateChannel, Purpose, SoftwareManifest,
    MAX_ENVELOPE_BYTES,
};
use rela_protocol::AppError;
use semver::Version;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

pub const DIRECTORY: &str = ".rela-update-control";
const RECORD: &str = "control.dat";

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BodyState {
    Waiting,
    Active,
    Committed,
    RolledBack,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CoreState {
    NotStarted,
    NotNeeded,
    Requested,
    Prepared,
    Committed,
    RolledBack,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Control {
    schema_version: u32,
    pub id: String,
    previous_version: Version,
    channel: UpdateChannel,
    envelope_digest: String,
    runner_digest: String,
    parent: ProcessTicket,
    handoff: Endpoint,
    pub runner: Option<ProcessTicket>,
    candidate: Option<ProcessTicket>,
    pub body: BodyState,
    core: CoreState,
    #[serde(default)]
    pub failure: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Handoff {
    Ready,
    Rejected,
}

/// Called from the original GUI after explicit confirmation. Its caller exits
/// the GUI only after this function returns successfully with a live controller.
pub async fn start(
    selected: SelectedUpdate,
    mut progress: impl FnMut(u64, u64),
) -> Result<(), AppError> {
    if platform::is_elevated() {
        return Err(AppError::new(
            "update_elevated_gui",
            "请先退出 Rela，再以普通用户方式启动后更新；需要更新网络引擎时会单独请求管理员授权。",
        ));
    }
    let executable = std::env::current_exe().map_err(|_| error())?;
    let root = executable.parent().ok_or_else(error)?;
    check_root(root)?;
    if !executable
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("Rela.exe"))
    {
        return Err(error());
    }
    if root
        .join(".rela-update")
        .try_exists()
        .map_err(|_| error())?
    {
        return Err(error());
    }
    let control = root.join(DIRECTORY);
    if control.try_exists().map_err(|_| error())? {
        // A prior incomplete preparation never changed live files. Preserve it for
        // diagnosis rather than guessing which unjournaled files can be deleted.
        package::plain_directory(&control)?;
        if control.join(RECORD).try_exists().map_err(|_| error())? {
            return Err(error());
        }
        let archive = root.join(format!(".rela-update-preparation-{}", ipc::random_id()?));
        fs::rename(&control, archive).map_err(|_| error())?;
    }
    fs::create_dir(&control).map_err(|_| error())?;
    let key = super::package_public_key()?;
    let mut downloaded = Fetcher::for_packages()?
        .download_package(&selected.manifest.portable, &key, &control, &mut progress)
        .await?;
    let mut package = downloaded.persist_copy(
        &control.join("update.zip"),
        &selected.manifest.portable,
        &key,
    )?;
    let stage = portable::stage(&mut package, &selected.manifest, &control.join("candidate"))?;
    stage.verify()?;
    platform::atomic_write(&control.join("software.json"), &selected.envelope)?;
    copy_runner(&executable, &control.join("runner.exe"))?;
    let id = ipc::random_id()?;
    let listener = Listener::bind(&id, Role::Controller)?;
    let state = Control {
        schema_version: 1,
        id: id.clone(),
        previous_version: Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| error())?,
        channel: selected.manifest.channel,
        envelope_digest: sha256(&selected.envelope),
        runner_digest: file_digest(&control.join("runner.exe"), 512 * 1024 * 1024)?,
        parent: ProcessFence::capture(std::process::id(), &executable)?,
        handoff: listener.endpoint(),
        runner: None,
        candidate: None,
        body: BodyState::Waiting,
        core: CoreState::NotStarted,
        failure: None,
    };
    save(root, &state)?;
    drop(package);
    drop(downloaded);
    // Spawn/IPC can wait for file verification, but must not block the GUI thread.
    let root = root.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || hand_off(&root, &id, listener, false))
        .await
        .map_err(|_| error())?
}

fn hand_off(root: &Path, id: &str, listener: Listener, recovering: bool) -> Result<(), AppError> {
    let runner = root.join(DIRECTORY).join("runner.exe");
    use std::os::windows::fs::OpenOptionsExt;
    package::plain_file(&runner)?;
    let _runner_guard = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&runner)
        .map_err(|_| error())?;
    let state = load(root)?;
    if state.id != id || file_digest(&runner, 512 * 1024 * 1024)? != state.runner_digest {
        return Err(error());
    }
    let mut child = Command::new(&runner)
        .args([
            if recovering {
                "--rela-portable-recover"
            } else {
                "--rela-portable-update"
            },
            id,
        ])
        .current_dir(root.join(DIRECTORY))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|_| error())?;
    let result = (|| {
        let mut channel = listener.accept(child.id(), &runner, Duration::from_secs(60), || {
            child.try_wait().is_ok_and(|status| status.is_some())
        })?;
        channel.timeout(Duration::from_secs(60))?;
        match channel.receive::<Handoff>()? {
            Handoff::Ready => Ok(()),
            Handoff::Rejected => Err(error()),
        }
    })();
    if result.is_err() {
        // This GUI has not exited. The controller is forbidden from changing any
        // live file before that exit, so only this exact spawned child is stopped.
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

pub fn entry() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mode = args.first().and_then(|arg| arg.to_str());
    if !matches!(
        mode,
        Some("--rela-portable-update" | "--rela-portable-recover")
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
        let control = executable.parent().ok_or_else(error)?;
        if control.file_name().is_none_or(|name| name != DIRECTORY)
            || executable
                .file_name()
                .is_none_or(|name| !name.to_string_lossy().eq_ignore_ascii_case("runner.exe"))
        {
            return Err(error());
        }
        run(
            control.parent().ok_or_else(error)?,
            id,
            mode == Some("--rela-portable-recover"),
        )
    })();
    if let Err(failure) = &result {
        platform::show_update_error(&failure.message);
    }
    Some(if result.is_ok() { 0 } else { 1 })
}

fn run(root: &Path, id: &str, recovering: bool) -> Result<(), AppError> {
    check_root(root)?;
    let mut state = load(root)?;
    if state.id != id
        || state.previous_version
            != Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| error())?
    {
        return Err(error());
    }
    let control = root.join(DIRECTORY);
    if file_digest(&control.join("runner.exe"), 512 * 1024 * 1024)? != state.runner_digest {
        return Err(error());
    }
    let (_package, manifest, stage) = verified(root, &state)?;
    let _stage_guard = stage.lock()?;
    let parent = ProcessFence::open(&state.parent, &root.join("Rela.exe"))?;
    if parent.has_exited()? {
        return Err(error());
    }
    let _update_permit = Permit::update()?;
    state.runner = Some(ProcessFence::capture(
        std::process::id(),
        &control.join("runner.exe"),
    )?);
    save(root, &state)?;
    let mut handoff = Channel::connect(&state.handoff)?;
    handoff.send(&Handoff::Ready)?;
    drop(handoff);
    parent.wait(Duration::from_secs(60))?;
    let lease = InstanceLease::acquire(&root.join("data/config"))?;
    let mut runtime = NativeRuntime {
        root: root.into(),
        state,
        core: None,
        child: None,
        channel: None,
        lease: Some(lease),
        version: manifest.version.clone(),
        index: stage.index().clone(),
        candidate_guard: None,
    };

    if !root
        .join(".rela-update")
        .try_exists()
        .map_err(|_| error())?
        && matches!(
            runtime.state.body,
            BodyState::Committed | BodyState::RolledBack
        )
    {
        // Cleanup finished before the last controller died; its sealed terminal
        // decision is sufficient, and no old snapshot is required for cleanup.
        if runtime.state.body == BodyState::Committed {
            portable::verify_program_files(root, stage.index())?;
            runtime.start_and_verify(&root.join("Rela.exe"))?;
            runtime.release()?;
        } else {
            runtime.launch_old()?;
        }
        return Ok(());
    }
    let mut transaction = if root
        .join(".rela-update")
        .try_exists()
        .map_err(|_| error())?
    {
        Transaction::reopen(root, &stage, id)?
    } else if recovering || runtime.state.body != BodyState::Waiting {
        // Waiting means no live file was changed; recovery can simply return to
        // the old GUI after validating its copied executable identity.
        runtime.state.body = BodyState::RolledBack;
        save(root, &runtime.state)?;
        runtime.launch_old()?;
        return Ok(());
    } else {
        Transaction::prepare(root, &stage, runtime.state.previous_version.clone(), id)?
    };
    let result = if recovering {
        transaction.recover(&mut runtime)
    } else {
        runtime.state.body = BodyState::Active;
        save(root, &runtime.state)?;
        transaction.execute(&mut runtime)
    };
    if let Err(failure) = &result {
        runtime.state.failure = Some(failure.message.clone());
        save(root, &runtime.state)?;
    }
    match transaction.phase() {
        Phase::Committed => {
            result?;
            runtime.state.body = BodyState::Committed;
            save(root, &runtime.state)?;
            if runtime.child.is_none() {
                runtime.start_and_verify(&root.join("Rela.exe"))?;
            }
            transaction.cleanup()?;
            runtime.release()?;
            Ok(())
        }
        Phase::RolledBack => {
            runtime.state.body = BodyState::RolledBack;
            save(root, &runtime.state)?;
            transaction.cleanup()?;
            runtime.launch_old()?;
            // Recovery success and a normal update rollback both restart the old GUI.
            // The restored GUI starts immediately; a blocking error dialog here
            // would keep this runner alive and prevent its startup cleanup.
            Ok(())
        }
        _ => result.and(Err(error())),
    }
}

struct NativeRuntime {
    root: PathBuf,
    state: Control,
    core: Option<core_worker::Client>,
    child: Option<Child>,
    channel: Option<Channel>,
    lease: Option<InstanceLease>,
    version: Version,
    index: portable::PortableIndex,
    candidate_guard: Option<portable::ProgramGuard>,
}
impl NativeRuntime {
    fn save(&self) -> Result<(), AppError> {
        save(&self.root, &self.state)
    }
    fn core(&mut self) -> Result<&mut core_worker::Client, AppError> {
        if self.core.is_none() {
            self.core = Some(core_worker::Client::launch(
                &self.root.join(DIRECTORY),
                &self.state.id,
            )?);
        }
        self.core.as_mut().ok_or_else(error)
    }
    fn release(&mut self) -> Result<(), AppError> {
        self.channel
            .as_mut()
            .ok_or_else(error)?
            .send(&StartupDecision::Release)?;
        self.channel.take();
        Ok(())
    }
    fn launch_old(&mut self) -> Result<(), AppError> {
        if file_digest(&self.root.join("Rela.exe"), 512 * 1024 * 1024)? != self.state.runner_digest
        {
            return Err(error());
        }
        self.lease.take();
        // --rela-restored still validates the sealed terminal transaction and waits
        // for this controller to exit before removing its leftover control files.
        Command::new(self.root.join("Rela.exe"))
            .args(["--rela-restored", &self.state.id])
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
            .map_err(|_| error())?;
        Ok(())
    }
}
impl Runtime for NativeRuntime {
    fn prepare(&mut self) -> Result<(), AppError> {
        if platform::service_state()? == platform::ServiceState::NotInstalled {
            self.state.core = CoreState::NotNeeded;
            return self.save();
        }
        self.state.core = CoreState::Requested;
        self.save()?;
        let connection = core_worker::Client::launch(&self.root.join(DIRECTORY), &self.state.id);
        match connection {
            Ok(client) => self.core = Some(client),
            Err(failure) => {
                self.state.core = CoreState::NotStarted;
                self.save()?;
                return Err(failure);
            }
        }
        let changed = self.core()?.prepare()?;
        self.state.core = if changed {
            CoreState::Prepared
        } else {
            CoreState::NotNeeded
        };
        self.save()
    }
    fn quiesce(&mut self) -> Result<(), AppError> {
        if let Some(channel) = &mut self.channel {
            let _ = channel.send(&StartupDecision::Abort);
        }
        self.channel.take();
        if let Some(mut child) = self.child.take() {
            if child.try_wait().map_err(|_| error())?.is_none() {
                child.kill().map_err(|_| error())?;
            }
            child.wait().map_err(|_| error())?;
        } else if let Some(ticket) = &self.state.candidate {
            // Never kill a PID recovered from disk. A surviving exact candidate
            // must exit when its controller channel closes, otherwise retry later.
            if let Ok(process) = ProcessFence::open(ticket, &self.root.join("Rela.exe")) {
                process.wait(Duration::from_secs(15))?;
            }
        }
        self.candidate_guard.take();
        self.state.candidate = None;
        self.save()?;
        if self.lease.is_none() {
            self.lease = Some(InstanceLease::acquire(&self.root.join("data/config"))?);
        }
        Ok(())
    }
    fn start_and_verify(&mut self, executable: &Path) -> Result<(), AppError> {
        self.candidate_guard = Some(portable::lock_program(&self.root, &self.index, false)?);
        let listener = Listener::bind(&self.state.id, Role::Candidate)?;
        let request = StartupRequest {
            endpoint: listener.endpoint(),
            controller: ProcessFence::capture(
                std::process::id(),
                &self.root.join(DIRECTORY).join("runner.exe"),
            )?,
            expected_version: self.version.clone(),
        };
        write_private(&self.root.join(DIRECTORY).join("startup.dat"), &request)?;
        self.lease.take();
        let child = Command::new(executable)
            .args(["--rela-updated", &self.state.id])
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
            .map_err(|_| error())?;
        let pid = child.id();
        self.child = Some(child);
        self.state.candidate = Some(ProcessFence::capture(pid, executable)?);
        self.save()?;
        let mut channel = listener.accept(pid, executable, Duration::from_secs(45), || {
            self.child
                .as_mut()
                .is_none_or(|child| child.try_wait().is_ok_and(|status| status.is_some()))
        })?;
        channel.timeout(Duration::from_secs(45))?;
        let ready: StartupReady = channel.receive()?;
        if ready.version != self.version {
            return Err(error());
        }
        self.channel = Some(channel);
        Ok(())
    }
    fn rollback(&mut self) -> Result<(), AppError> {
        if self.state.core == CoreState::NotNeeded && self.core.is_some() {
            self.core()?.rollback()?;
            self.core.take();
        }
        if matches!(self.state.core, CoreState::Requested | CoreState::Prepared) {
            self.core()?.rollback()?;
            self.state.core = CoreState::RolledBack;
            self.save()?;
        } else if self.state.core == CoreState::Committed {
            return Err(error());
        }
        Ok(())
    }
    fn commit(&mut self) -> Result<(), AppError> {
        if self.state.core == CoreState::NotNeeded && self.core.is_some() {
            self.core()?.commit()?;
            self.core.take();
        }
        if matches!(
            self.state.core,
            CoreState::Requested | CoreState::Prepared | CoreState::Committed
        ) {
            self.core()?.commit()?;
            self.state.core = CoreState::Committed;
            self.save()?;
        }
        Ok(())
    }
}

pub(crate) fn load(root: &Path) -> Result<Control, AppError> {
    let record: Control = read_private(&root.join(DIRECTORY).join(RECORD))?;
    if record.schema_version != 1 || !record.previous_version.build.is_empty() {
        return Err(error());
    }
    ipc::validate_id(&record.id)?;
    ipc::validate_id(&record.envelope_digest)?;
    ipc::validate_id(&record.runner_digest)?;
    record.handoff.validate()?;
    if record.handoff.id != record.id || record.handoff.role != Role::Controller {
        return Err(error());
    }
    Ok(record)
}
fn save(root: &Path, record: &Control) -> Result<(), AppError> {
    write_private(&root.join(DIRECTORY).join(RECORD), record)
}
pub(crate) fn write_private(path: &Path, value: &impl Serialize) -> Result<(), AppError> {
    package::plain_directory(path.parent().ok_or_else(error)?)?;
    if path.try_exists().map_err(|_| error())? {
        package::plain_file(path)?;
    }
    let encrypted = platform::protect(&serde_json::to_vec(value).map_err(|_| error())?)?;
    platform::atomic_write(path, &encrypted)?;
    platform::private_file(path)
}
pub(crate) fn read_private<T: DeserializeOwned>(path: &Path) -> Result<T, AppError> {
    let bytes = read(path, 65536)?;
    serde_json::from_slice(&platform::unprotect(&bytes)?).map_err(|_| error())
}
fn verified(
    root: &Path,
    record: &Control,
) -> Result<(VerifiedPackage, SoftwareManifest, PortableStage), AppError> {
    verified_using(
        root,
        record,
        &DistributionConfig::bundled()?,
        &super::package_public_key()?,
    )
}
fn verified_using(
    root: &Path,
    record: &Control,
    config: &DistributionConfig,
    key: &str,
) -> Result<(VerifiedPackage, SoftwareManifest, PortableStage), AppError> {
    let control = root.join(DIRECTORY);
    let envelope = read(&control.join("software.json"), MAX_ENVELOPE_BYTES as u64)?;
    if sha256(&envelope) != record.envelope_digest {
        return Err(error());
    }
    let payload = verify_envelope(&envelope, Purpose::Software, &config.keys)
        .map_err(distribution::manifest_error)?;
    let manifest =
        SoftwareManifest::parse(&payload, record.channel).map_err(distribution::manifest_error)?;
    if !manifest
        .newer_than(&record.previous_version)
        .map_err(distribution::manifest_error)?
    {
        return Err(error());
    }
    let mut package = VerifiedPackage::open(&control.join("update.zip"), &manifest.portable, key)?;
    let stage = portable::reopen_stage(&mut package, &manifest, &control.join("candidate"))?;
    Ok((package, manifest, stage))
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>, AppError> {
    package::plain_file(path)?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| error())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error())?;
    if bytes.len() as u64 > limit {
        return Err(error());
    }
    Ok(bytes)
}
fn file_digest(path: &Path, limit: u64) -> Result<String, AppError> {
    use sha2::{Digest, Sha256};
    package::plain_file(path)?;
    let mut hash = Sha256::new();
    let copied = std::io::copy(
        &mut File::open(path).map_err(|_| error())?.take(limit + 1),
        &mut hash,
    )
    .map_err(|_| error())?;
    if copied > limit {
        return Err(error());
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub(super) fn copy_runner(source: &Path, target: &Path) -> Result<(), AppError> {
    package::plain_file(source)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)
        .map_err(|_| error())?;
    let copied = std::io::copy(
        &mut File::open(source)
            .map_err(|_| error())?
            .take(512 * 1024 * 1024 + 1),
        &mut output,
    )
    .map_err(|_| error())?;
    output
        .flush()
        .and_then(|_| output.sync_all())
        .map_err(|_| error())?;
    drop(output);
    if copied > 512 * 1024 * 1024
        || file_digest(source, 512 * 1024 * 1024)? != file_digest(target, 512 * 1024 * 1024)?
    {
        return Err(error());
    }
    Ok(())
}
/// Called with the configuration instance lease, before any config migration.
pub fn resume_before_gui(root: &Path) -> Result<bool, AppError> {
    check_root(root)?;
    let control = root.join(DIRECTORY);
    if !control.join(RECORD).try_exists().map_err(|_| error())? {
        if root
            .join(".rela-update")
            .try_exists()
            .map_err(|_| error())?
        {
            return Err(error());
        }
        return Ok(false);
    }
    let mut state = load(root)?;
    if let Some(ticket) = &state.runner {
        if ProcessFence::open(ticket, &control.join("runner.exe"))
            .is_ok_and(|p| p.has_exited().is_ok_and(|v| !v))
        {
            return Err(error());
        }
    }
    let (_package, _, _stage) = verified(root, &state)?;
    if !root
        .join(".rela-update")
        .try_exists()
        .map_err(|_| error())?
        && matches!(state.body, BodyState::Committed | BodyState::RolledBack)
    {
        drop(_package);
        archive_terminal(root, &state.id, state.body)?;
        return Ok(false);
    }
    // A crash while copying snapshots precedes the first file/Core mutation.
    if state.body == BodyState::Waiting
        && state.core == CoreState::NotStarted
        && root
            .join(".rela-update")
            .try_exists()
            .map_err(|_| error())?
        && !root
            .join(".rela-update/journal.json")
            .try_exists()
            .map_err(|_| error())?
    {
        if file_digest(&root.join("Rela.exe"), 512 * 1024 * 1024)? != state.runner_digest {
            return Err(error());
        }
        package::plain_directory(&root.join(".rela-update"))?;
        fs::rename(
            root.join(".rela-update"),
            root.join(format!(".rela-update-incomplete-{}", state.id)),
        )
        .map_err(|_| error())?;
    }
    let listener = Listener::bind(&state.id, Role::Controller)?;
    state.parent = ProcessFence::capture(std::process::id(), &root.join("Rela.exe"))?;
    state.handoff = listener.endpoint();
    save(root, &state)?;
    hand_off(root, &state.id, listener, true)?;
    Ok(true)
}

pub fn restored(root: &Path, id: &str) -> Result<(), AppError> {
    let state = load(root)?;
    if state.id != id || state.body != BodyState::RolledBack {
        return Err(error());
    }
    if let Some(ticket) = &state.runner {
        if let Ok(process) = ProcessFence::open(ticket, &root.join(DIRECTORY).join("runner.exe")) {
            process.wait(Duration::from_secs(15))?;
        }
    }
    archive_terminal(root, id, BodyState::RolledBack)
}

/// Atomically retire terminal control state only after the runner exits. Keep the
/// verified staging directory as a recovery archive; no user file is deleted.
pub(crate) fn archive_terminal(root: &Path, id: &str, expected: BodyState) -> Result<(), AppError> {
    let state = load(root)?;
    if state.id != id
        || state.body != expected
        || !matches!(expected, BodyState::Committed | BodyState::RolledBack)
        || root
            .join(".rela-update")
            .try_exists()
            .map_err(|_| error())?
    {
        return Err(error());
    }
    if let Some(ticket) = &state.runner {
        if ProcessFence::open(ticket, &root.join(DIRECTORY).join("runner.exe"))
            .is_ok_and(|p| p.has_exited().is_ok_and(|v| !v))
        {
            return Err(error());
        }
    }
    let (package, _, stage) = verified(root, &state)?;
    if expected == BodyState::Committed {
        portable::verify_program_files(root, stage.index())?;
    } else if file_digest(&root.join("Rela.exe"), 512 * 1024 * 1024)? != state.runner_digest {
        return Err(error());
    }
    drop(package);
    fs::rename(
        root.join(DIRECTORY),
        root.join(format!(".rela-update-finished-{id}")),
    )
    .map_err(|_| error())
}

fn check_root(root: &Path) -> Result<(), AppError> {
    package::plain_directory(root)?;
    package::plain_file(&root.join("portable.txt"))
}
pub(crate) fn error() -> AppError {
    AppError::new(
        "portable_update_recovery",
        "更新尚未完成，恢复文件已保留。请退出其他 Rela 副本后重试，或联系管理员恢复。",
    )
}

#[cfg(test)]
mod tests;
