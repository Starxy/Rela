//! Signed NSIS PRE/POST hooks and the elevated recovery participant. The NSIS
//! script calls these synchronously; POST remains blocked until the controller
//! has verified the candidate and finished the initiating user's config work.
use super::{
    installed::{self, guard, native::Native, Phase, SystemState, Transaction},
    ipc::{self, Channel, Endpoint, Role},
    portable::{exists, fingerprint, read, MAX_PROGRAM},
    process::{ProcessFence, ProcessTicket},
};
use crate::{
    distribution::{
        self,
        package::{self, VerifiedPackage},
        portable::{self, PortableStage},
        DistributionConfig,
    },
    easytier::update_participant::CoreUpdateParticipant,
    platform,
};
use rela_manifests::{
    sha256, verify_envelope, Channel as UpdateChannel, Purpose, SoftwareManifest,
    MAX_ENVELOPE_BYTES,
};
use rela_protocol::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};

pub const REQUEST_FILE: &str = "installed-request.dat";
const STATE_FILE: &str = "participant.json";

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub id: String,
    pub root: PathBuf,
    pub previous_version: Version,
    pub channel: UpdateChannel,
    pub envelope_digest: String,
    pub runner_digest: String,
    pub controller: ProcessTicket,
    pub pre: Endpoint,
    pub post: Endpoint,
    pub recovery: Option<Endpoint>,
}
impl Request {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != 1
            || !self.root.is_absolute()
            || !self.previous_version.build.is_empty()
            || self.controller.pid == 0
            || self
                .controller
                .started_at
                .parse::<u64>()
                .ok()
                .filter(|v| *v > 0)
                .is_none()
        {
            return Err(error());
        }
        for value in [&self.id, &self.envelope_digest, &self.runner_digest] {
            ipc::validate_id(value)?;
        }
        for (endpoint, role) in [
            (&self.pre, Role::InstallerPre),
            (&self.post, Role::InstallerPost),
        ] {
            endpoint.validate()?;
            if endpoint.id != self.id || endpoint.role != role {
                return Err(error());
            }
        }
        if let Some(endpoint) = &self.recovery {
            endpoint.validate()?;
            if endpoint.id != self.id || endpoint.role != Role::InstallerRecovery {
                return Err(error());
            }
        }
        Ok(())
    }
    fn same_selection(&self, previous: &Self) -> bool {
        self.id == previous.id
            && self.root == previous.root
            && self.previous_version == previous.previous_version
            && self.channel == previous.channel
            && self.envelope_digest == previous.envelope_digest
            && self.runner_digest == previous.runner_digest
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub installer: Option<ProcessTicket>,
    pub installer_image: Option<PathBuf>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Prepare,
    AllowInstall,
    Commit,
    Rollback,
    Finalize,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Unstarted,
    BackedUp,
    Ready { phase: Phase },
    Resolved { phase: Phase },
    Finished { phase: Phase },
    Failed,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CoreState {
    NotStarted,
    Requested,
    NotNeeded,
    Prepared,
    Committed,
    RolledBack,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    request: Request,
    installer: ProcessTicket,
    installer_image: PathBuf,
    authorized: bool,
    core: CoreState,
}

pub struct VerifiedSource {
    pub manifest: SoftwareManifest,
    pub stage: PortableStage,
    package: VerifiedPackage,
    envelope: Vec<u8>,
}
pub fn read_request(path: &Path) -> Result<Request, AppError> {
    let record: Request =
        serde_json::from_slice(&platform::unprotect(&read(path, 65536)?)?).map_err(|_| error())?;
    record.validate()?;
    let control = path.parent().ok_or_else(error)?;
    if path.file_name().is_none_or(|name| name != REQUEST_FILE)
        || control
            .file_name()
            .is_none_or(|name| name != format!(".rela-installed-user-{}", record.id).as_str())
    {
        return Err(error());
    }
    Ok(record)
}
pub fn write_request(control: &Path, request: &Request) -> Result<(), AppError> {
    request.validate()?;
    package::plain_directory(control)?;
    if control
        .file_name()
        .is_none_or(|name| name != format!(".rela-installed-user-{}", request.id).as_str())
    {
        return Err(error());
    }
    let path = control.join(REQUEST_FILE);
    platform::atomic_write(
        &path,
        &platform::protect_request(&serde_json::to_vec(request).map_err(|_| error())?)?,
    )?;
    platform::private_file(&path)
}
pub fn verify_source(control: &Path, request: &Request) -> Result<VerifiedSource, AppError> {
    verify_source_using(
        control,
        request,
        &DistributionConfig::bundled()?,
        &super::package_public_key()?,
    )
}
pub(super) fn verify_source_using(
    control: &Path,
    request: &Request,
    distribution: &DistributionConfig,
    key: &str,
) -> Result<VerifiedSource, AppError> {
    request.validate()?;
    let envelope = read(&control.join("software.json"), MAX_ENVELOPE_BYTES as u64)?;
    if sha256(&envelope) != request.envelope_digest {
        return Err(error());
    }
    let payload = verify_envelope(&envelope, Purpose::Software, &distribution.keys)
        .map_err(distribution::manifest_error)?;
    let manifest =
        SoftwareManifest::parse(&payload, request.channel).map_err(distribution::manifest_error)?;
    if !manifest
        .newer_than(&request.previous_version)
        .map_err(distribution::manifest_error)?
    {
        return Err(error());
    }
    let mut package = VerifiedPackage::open(&control.join("update.zip"), &manifest.portable, key)?;
    let stage = portable::reopen_stage(&mut package, &manifest, &control.join("candidate"))?;
    Ok(VerifiedSource {
        manifest,
        stage,
        package,
        envelope,
    })
}

struct Machine {
    root: PathBuf,
    control: PathBuf,
    record: Record,
    source: VerifiedSource,
}
impl Machine {
    fn prepare(
        request: &Request,
        mut source: VerifiedSource,
        installer: ProcessTicket,
        installer_image: PathBuf,
        system: &Native,
    ) -> Result<Self, AppError> {
        let root = request.root.canonicalize().map_err(|_| error())?;
        // Complete the protected source before publishing the fixed pending
        // marker. A failed initial copy leaves only an inert retained staging
        // directory; neither application nor installer files have been changed.
        let control = root.join(format!(".rela-installed-staging-{}", request.id));
        system.create_control(&control, &request.id)?;
        platform::atomic_write(&control.join("software.json"), &source.envelope)?;
        let key = super::package_public_key()?;
        let mut package = source.package.persist_copy(
            &control.join("update.zip"),
            &source.manifest.portable,
            &key,
        )?;
        let stage = portable::stage(&mut package, &source.manifest, &control.join("candidate"))?;
        let result = Self {
            root,
            control,
            record: Record {
                schema_version: 1,
                request: request.clone(),
                installer,
                installer_image,
                authorized: false,
                core: CoreState::NotStarted,
            },
            source: VerifiedSource {
                manifest: source.manifest,
                envelope: source.envelope,
                package,
                stage,
            },
        };
        result.save()?;
        drop(result.source);
        platform::move_file_new(&result.control, &result.root.join(guard::CONTROL))?;
        Self::reopen(request, system)
    }
    fn reopen(request: &Request, system: &Native) -> Result<Self, AppError> {
        let root = request.root.canonicalize().map_err(|_| error())?;
        let control = control_location(&root, &request.id)?;
        system.validate_control(&control, &request.id)?;
        system.validate_live_file(&control.join(STATE_FILE))?;
        let record: Record = serde_json::from_slice(&read(&control.join(STATE_FILE), 65536)?)
            .map_err(|_| error())?;
        record.request.validate()?;
        if record.schema_version != 1
            || !request.same_selection(&record.request)
            || (request.recovery.is_none() && *request != record.request)
            || record.request.root != root
            || !record.installer_image.is_absolute()
        {
            return Err(error());
        }
        let source = verify_source(&control, &record.request)?;
        Ok(Self {
            root,
            control,
            record,
            source,
        })
    }
    fn save(&self) -> Result<(), AppError> {
        platform::atomic_write(
            &self.control.join(STATE_FILE),
            &serde_json::to_vec(&self.record).map_err(|_| error())?,
        )
    }
    fn archive(self) -> Result<(), AppError> {
        let archived = archive_location(&self.root, &self.record.request.id);
        // Neither this NSIS hook nor a recovery entry executes from the retained
        // machine snapshot. Release verification handles before retiring it.
        drop(self.source);
        if self.control == archived {
            Ok(())
        } else {
            platform::move_file_new(&self.control, &archived)
        }
    }
}

fn archive_location(root: &Path, id: &str) -> PathBuf {
    root.join(format!(".rela-installed-finished-{id}"))
}

fn control_location(root: &Path, id: &str) -> Result<PathBuf, AppError> {
    ipc::validate_id(id)?;
    let active = root.join(guard::CONTROL);
    // An active transaction always takes precedence, including another id.
    // Reopening it checks identity instead of mutating an older archive.
    Ok(if exists(&active)? {
        active
    } else {
        archive_location(root, id)
    })
}

pub fn entry() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let mode = args.first().and_then(|arg| arg.to_str());
    if !matches!(
        mode,
        Some(
            "--rela-nsis-pre"
                | "--rela-nsis-post"
                | "--rela-installed-recover"
                | "--rela-nsis-maintenance"
        )
    ) {
        return None;
    }
    let result = match mode {
        Some("--rela-nsis-maintenance") => maintenance(&args),
        Some("--rela-nsis-pre") => pre(&args),
        Some("--rela-nsis-post") => post_or_recover(&args, false),
        _ => post_or_recover(&args, true),
    };
    Some(match result {
        Ok(committed) => {
            if committed {
                0
            } else {
                2
            }
        }
        Err(_) => 2,
    })
}

fn maintenance(args: &[OsString]) -> Result<bool, AppError> {
    if args.len() != 2 || !platform::is_elevated() {
        return Err(error());
    }
    let root = Path::new(&args[1]);
    let system = Native::open(root)?;
    if guard::pending_update(root)? {
        return Err(error());
    }
    let version = Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| error())?;
    system.snapshot()?.validate_installation(root, &version)?;
    // The NSIS-extracted helper must be exactly its installed bundled executable.
    if fingerprint(&std::env::current_exe().map_err(|_| error())?, MAX_PROGRAM)?
        != fingerprint(&root.join("Rela.exe"), MAX_PROGRAM)?
    {
        return Err(error());
    }
    system.ensure_running_lock()?;
    Ok(true)
}

fn input(
    args: &[OsString],
    recovering: bool,
) -> Result<(Request, PathBuf, VerifiedSource, ProcessFence), AppError> {
    if !platform::is_elevated() || args.len() != if recovering { 2 } else { 4 } {
        return Err(error());
    }
    let request_path = Path::new(&args[1]);
    let request = read_request(request_path)?;
    if request.recovery.is_some() != recovering {
        return Err(error());
    }
    package::plain_directory(&request.root)?;
    if request.root.canonicalize().map_err(|_| error())? != request.root
        || exists(&request.root.join("portable.txt"))?
        || (!recovering && Path::new(&args[2]).canonicalize().map_err(|_| error())? != request.root)
    {
        return Err(error());
    }
    let control = request_path.parent().ok_or_else(error)?.to_path_buf();
    let parent = ProcessFence::open(&request.controller, &control.join("runner.exe"))?;
    if parent.has_exited()?
        || fingerprint(&control.join("runner.exe"), MAX_PROGRAM)?.sha256 != request.runner_digest
    {
        return Err(error());
    }
    let source = verify_source(&control, &request)?;
    if source.manifest.version != Version::parse(env!("CARGO_PKG_VERSION")).map_err(|_| error())?
        || fingerprint(&std::env::current_exe().map_err(|_| error())?, MAX_PROGRAM)?.sha256
            != source.stage.index().files["Rela.exe"]
    {
        return Err(error());
    }
    Ok((request, control, source, parent))
}

fn installer(
    args: &[OsString],
    source: &VerifiedSource,
) -> Result<(ProcessFence, ProcessTicket, PathBuf, VerifiedPackage), AppError> {
    let pid = args[3]
        .to_str()
        .ok_or_else(error)?
        .parse::<u32>()
        .map_err(|_| error())?;
    let (process, ticket, image) = ProcessFence::capture_image(pid)?;
    let (myself, _, _) = ProcessFence::capture_image(std::process::id())?;
    myself.is_child_of(std::process::id(), &ticket)?;
    let package = VerifiedPackage::open_versioned(
        &image,
        &source.manifest.installer,
        &super::package_public_key()?,
        &source.manifest.version,
    )?;
    Ok((process, ticket, image, package))
}

fn pre(args: &[OsString]) -> Result<bool, AppError> {
    let (request, _, source, parent) = input(args, false)?;
    let (installer, ticket, image, _installer_bytes) = installer(args, &source)?;
    let _source_guard = source.stage.lock()?;
    let mut channel = Channel::connect(&request.pre)?;
    channel.send(&Hello {
        installer: Some(ticket.clone()),
        installer_image: Some(image.clone()),
    })?;
    if channel.receive::<Command>()? != Command::Prepare
        || parent.has_exited()?
        || installer.has_exited()?
    {
        return Err(error());
    }
    let system = Native::open(&request.root)?;
    let _gui_fence = guard::Lease::exclusive(&request.root)?;
    if guard::pending_update(&request.root)?
        || fingerprint(&request.root.join("Rela.exe"), MAX_PROGRAM)?.sha256 != request.runner_digest
    {
        return Err(error());
    }
    let mut machine = Machine::prepare(&request, source, ticket, image, &system)?;
    let _body = Transaction::prepare(
        &request.root,
        &machine.source.stage,
        request.previous_version.clone(),
        &request.id,
        &system,
    )?;
    channel.send(&Response::BackedUp)?;
    if channel.receive::<Command>()? != Command::AllowInstall
        || parent.has_exited()?
        || installer.has_exited()?
    {
        return Err(error());
    }
    // The script may execute File instructions only after this durable flag and
    // this process's successful exit. A crash/nonzero status aborts PREINSTALL.
    machine.record.authorized = true;
    machine.save()?;
    Ok(true)
}

trait Core {
    fn prepare(&mut self, bundled: &Path) -> Result<bool, AppError>;
    fn commit(&mut self) -> Result<(), AppError>;
    fn rollback(&mut self) -> Result<(), AppError>;
}
struct NativeCore {
    id: String,
    participant: Option<CoreUpdateParticipant>,
}
impl NativeCore {
    fn participant(&mut self) -> Result<&mut CoreUpdateParticipant, AppError> {
        if self.participant.is_none() {
            self.participant = Some(CoreUpdateParticipant::resume(&self.id)?);
        }
        self.participant.as_mut().ok_or_else(error)
    }
}
impl Core for NativeCore {
    fn prepare(&mut self, bundled: &Path) -> Result<bool, AppError> {
        self.participant()?.prepare_update(bundled)
    }
    fn commit(&mut self) -> Result<(), AppError> {
        self.participant()?.commit()
    }
    fn rollback(&mut self) -> Result<(), AppError> {
        self.participant()?.rollback()
    }
}

/// The durable body phase is authoritative. No IPC timeout chooses a decision.
fn resolve(
    body: &mut Transaction,
    system: &mut impl SystemState,
    core: &mut impl Core,
    core_state: CoreState,
    decision: Command,
) -> Result<CoreState, AppError> {
    match decision {
        Command::Commit => {
            if !matches!(
                body.phase(),
                Phase::Installed | Phase::Committing | Phase::Committed
            ) || !matches!(
                core_state,
                CoreState::NotNeeded | CoreState::Prepared | CoreState::Committed
            ) || (body.phase() == Phase::Installed && core_state == CoreState::Committed)
            {
                return Err(error());
            }
            if body.phase() != Phase::Committed {
                body.begin_commit(system)?;
            }
            if matches!(core_state, CoreState::Prepared | CoreState::Committed) {
                core.commit()?;
            }
            body.finish_commit(system)?;
            Ok(if core_state == CoreState::NotNeeded {
                core_state
            } else {
                CoreState::Committed
            })
        }
        Command::Rollback => {
            if matches!(body.phase(), Phase::Committing | Phase::Committed)
                || core_state == CoreState::Committed
            {
                return Err(error());
            }
            body.rollback(system)?;
            if matches!(
                core_state,
                CoreState::Requested | CoreState::Prepared | CoreState::RolledBack
            ) {
                core.rollback()?;
            }
            Ok(
                if core_state == CoreState::NotStarted || core_state == CoreState::NotNeeded {
                    core_state
                } else {
                    CoreState::RolledBack
                },
            )
        }
        _ => Err(error()),
    }
}

fn may_finalize(body: Phase, core: CoreState) -> bool {
    matches!(
        (body, core),
        (
            Phase::Committed,
            CoreState::NotNeeded | CoreState::Committed
        ) | (
            Phase::RolledBack,
            CoreState::NotStarted | CoreState::NotNeeded | CoreState::RolledBack
        )
    )
}

fn reopen_body(
    root: &Path,
    stage: &PortableStage,
    record: &Record,
    system: &impl SystemState,
    recovering: bool,
) -> Result<Transaction, AppError> {
    let id = &record.request.id;
    match Transaction::reopen(root, stage, id, system) {
        Ok(body) => Ok(body),
        Err(failure) => {
            // NSIS cannot write until PRE returned success, which requires the
            // durable authorization bit. Only an interrupted preparation with
            // no complete body journal is eligible for this path. Keep partial
            // backups intact and rebuild the transaction from unchanged files.
            let session = root.join(installed::DIRECTORY);
            if !recovering
                || record.authorized
                || record.core != CoreState::NotStarted
                || exists(&session.join("journal.json"))?
                || exists(&installed::receipt_path(root, id))?
                || fingerprint(&root.join("Rela.exe"), MAX_PROGRAM)?.sha256
                    != record.request.runner_digest
            {
                return Err(failure);
            }
            system.validate_root(root)?;
            system
                .snapshot()?
                .validate_installation(root, &record.request.previous_version)?;
            if exists(&session)? {
                system.validate_session(&session)?;
                let retained = root.join(format!(
                    ".rela-installed-preparation-{id}-{}",
                    ipc::random_id()?
                ));
                platform::move_file_new(&session, &retained)?;
            }
            Transaction::prepare(
                root,
                stage,
                record.request.previous_version.clone(),
                id,
                system,
            )
        }
    }
}

fn post_or_recover(args: &[OsString], recovering: bool) -> Result<bool, AppError> {
    let (request, _, source, parent) = input(args, recovering)?;
    let _source_guard = source.stage.lock()?;
    // Persist the worker identity before allowing recovery to rebuild backups.
    let recovery_channel = if recovering {
        let mut channel = Channel::connect(request.recovery.as_ref().ok_or_else(error)?)?;
        channel.send(&Hello {
            installer: None,
            installer_image: None,
        })?;
        if channel.receive::<Command>()? != Command::Prepare || parent.has_exited()? {
            return Err(error());
        }
        Some(channel)
    } else {
        None
    };
    let mut system = Native::open(&request.root)?;
    if recovering
        && !exists(&request.root.join(guard::CONTROL))?
        && !exists(&request.root.join(installed::DIRECTORY))?
        && !exists(&archive_location(&request.root, &request.id))?
        && !exists(&installed::receipt_path(&request.root, &request.id))?
    {
        // No PRE authorization could have completed without these protected
        // records. Exclude a still-preparing PRE worker and recheck under lock.
        let _lease = guard::Lease::exclusive(&request.root)?;
        if guard::pending_update(&request.root)?
            || exists(&archive_location(&request.root, &request.id))?
            || exists(&installed::receipt_path(&request.root, &request.id))?
            || fingerprint(&request.root.join("Rela.exe"), MAX_PROGRAM)?.sha256
                != request.runner_digest
        {
            return Err(error());
        }
        system
            .snapshot()?
            .validate_installation(&request.root, &request.previous_version)?;
        let mut channel = recovery_channel.ok_or_else(error)?;
        channel.send(&Response::Unstarted)?;
        if channel.receive::<Command>()? != Command::Finalize || parent.has_exited()? {
            return Err(error());
        }
        channel.send(&Response::Finished {
            phase: Phase::RolledBack,
        })?;
        return Ok(false);
    }
    let mut machine = Machine::reopen(&request, &system)?;
    let nsis = if recovering {
        if let Some(process) = ProcessFence::probe(&machine.record.installer)? {
            process.wait(Duration::from_secs(60))?;
        }
        None
    } else {
        let value = installer(args, &source)?;
        if !machine.record.authorized
            || value.1 != machine.record.installer
            || value.2 != machine.record.installer_image
        {
            return Err(error());
        }
        Some(value)
    };
    let mut channel = if let Some(channel) = recovery_channel {
        channel
    } else {
        let mut channel = Channel::connect(&request.post)?;
        channel.send(&Hello {
            installer: nsis.as_ref().map(|value| value.1.clone()),
            installer_image: nsis.as_ref().map(|value| value.2.clone()),
        })?;
        channel
    };
    let mut body = reopen_body(
        &request.root,
        &machine.source.stage,
        &machine.record,
        &system,
        recovering,
    )?;
    let mut core = NativeCore {
        id: request.id.clone(),
        participant: None,
    };
    if !recovering {
        if channel.receive::<Command>()? != Command::Prepare || parent.has_exited()? {
            return Err(error());
        }
        body.installed(&system)?;
        machine.record.core = CoreState::Requested;
        machine.save()?;
        let result = core.prepare(&machine.source.stage.directory().join("easytier"));
        match result {
            Ok(changed) => {
                machine.record.core = if changed {
                    CoreState::Prepared
                } else {
                    CoreState::NotNeeded
                };
                machine.save()?;
                channel.send(&Response::Ready {
                    phase: body.phase(),
                })?;
            }
            Err(_) => channel.send(&Response::Failed)?,
        }
    } else {
        channel.send(&Response::Ready {
            phase: body.phase(),
        })?;
    }
    loop {
        let command: Command = channel.receive()?;
        if parent.has_exited()? {
            return Err(error());
        }
        match command {
            Command::Commit | Command::Rollback => {
                // Rollback overwrites the live program and must exclude even
                // a still-running authenticated candidate from any controller.
                let _gui_fence = if command == Command::Rollback {
                    Some(guard::Lease::exclusive(&request.root)?)
                } else {
                    None
                };
                match resolve(
                    &mut body,
                    &mut system,
                    &mut core,
                    machine.record.core,
                    command,
                ) {
                    Ok(state) => {
                        machine.record.core = state;
                        machine.save()?;
                        channel.send(&Response::Resolved {
                            phase: body.phase(),
                        })?;
                    }
                    Err(_) => channel.send(&Response::Failed)?,
                }
            }
            Command::Finalize if matches!(body.phase(), Phase::Committed | Phase::RolledBack) => {
                // Config has been resolved in the original user's process. Core
                // and body decisions must already agree before cleanup is allowed.
                if !may_finalize(body.phase(), machine.record.core) {
                    return Err(error());
                }
                body.cleanup(&system)?;
                let phase = body.phase();
                drop(core);
                machine.archive()?;
                channel.send(&Response::Finished { phase })?;
                return Ok(phase == Phase::Committed);
            }
            _ => channel.send(&Response::Failed)?,
        }
    }
}

pub fn error() -> AppError {
    AppError::new(
        "installed_update_pending",
        "安装版更新尚未完成，请从发起更新的 Rela 副本重试恢复。备份已保留。",
    )
}

#[cfg(test)]
mod tests;
