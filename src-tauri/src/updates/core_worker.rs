//! Restricted elevated Core worker. No GUI, arbitrary commands or config input.
use super::{
    ipc::{self, Channel, Endpoint, Listener, Role},
    process::{ProcessFence, ProcessTicket},
};
use crate::{distribution::package, easytier::update_participant::CoreUpdateParticipant, platform};
use rela_protocol::AppError;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const REQUEST_FILE: &str = "core-request.dat";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    endpoint: Endpoint,
    controller: ProcessTicket,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Command {
    Prepare,
    Commit,
    Rollback,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Response {
    Prepared { changed_service: bool },
    Complete,
    Failed { code: Failure },
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Failure {
    Integrity,
    Conflict,
    Downgrade,
    Recovery,
    Migration,
    Pending,
    Permission,
    Other,
}
impl Failure {
    fn from_error(error: &AppError) -> Self {
        match error.code.as_str() {
            "core_integrity_failed" => Self::Integrity,
            "service_conflict" => Self::Conflict,
            "core_downgrade_blocked" => Self::Downgrade,
            "core_recovery_required" => Self::Recovery,
            "credential_migration_required" => Self::Migration,
            "core_application_update_pending" => Self::Pending,
            "elevation_required" => Self::Permission,
            _ => Self::Other,
        }
    }
    fn error(&self) -> AppError {
        let (code, message) = match self {
            Self::Integrity => (
                "core_integrity_failed",
                "网络引擎校验失败，已保留更新恢复状态。",
            ),
            Self::Conflict => (
                "service_conflict",
                "网络服务或管理端口冲突，请关闭冲突程序后重试。",
            ),
            Self::Downgrade => (
                "core_downgrade_blocked",
                "此副本不能覆盖较新的网络引擎，请使用较新 Rela。",
            ),
            Self::Recovery | Self::Other => (
                "core_recovery_required",
                "网络引擎更新尚未恢复，请重试或联系管理员。",
            ),
            Self::Migration => (
                "credential_migration_required",
                "请先完成 credential 配置迁移，再更新网络引擎。",
            ),
            Self::Pending => (
                "core_application_update_pending",
                "软件更新尚未完成，请使用发起更新的 Rela 副本恢复。",
            ),
            Self::Permission => ("elevation_required", "更新网络引擎需要管理员授权。"),
        };
        AppError::new(code, message)
    }
}

pub struct Client {
    channel: Channel,
    process: platform::ElevatedProcess,
}
impl Client {
    /// The caller is the non-elevated runner at control/runner.exe. The candidate
    /// executable must already have been validated against the signed update ZIP.
    pub fn launch(control: &Path, id: &str) -> Result<Self, AppError> {
        package::plain_directory(control)?;
        let runner = control.join("runner.exe");
        let worker = control.join("candidate/Rela.exe");
        package::plain_file(&runner)?;
        package::plain_file(&worker)?;
        let controller = ProcessFence::capture(std::process::id(), &runner)?;
        let listener = Listener::bind(id, Role::Core)?;
        let path = control.join(REQUEST_FILE);
        if path.try_exists().map_err(|_| ipc::protocol_error())? {
            package::plain_file(&path)?;
            fs::remove_file(&path).map_err(|_| ipc::protocol_error())?;
        }
        let encrypted = platform::protect_request(
            &serde_json::to_vec(&Request {
                endpoint: listener.endpoint(),
                controller,
            })
            .map_err(|_| ipc::protocol_error())?,
        )?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|_| ipc::protocol_error())?;
        platform::private_file(&path)?;
        file.write_all(&encrypted)
            .and_then(|_| file.sync_all())
            .map_err(|_| ipc::protocol_error())?;
        drop(file);
        let process = platform::launch_core_update_helper(&worker, &path)?;
        let channel = listener.accept(process.id(), &worker, Duration::from_secs(30), || {
            process.has_exited()
        })?;
        // The authenticated worker has read the request; do not retain the IPC token.
        fs::remove_file(path).map_err(|_| ipc::protocol_error())?;
        Ok(Self { channel, process })
    }
    pub fn prepare(&mut self) -> Result<bool, AppError> {
        self.channel.send(&Command::Prepare)?;
        match self.channel.receive()? {
            Response::Prepared { changed_service } => Ok(changed_service),
            Response::Failed { code } => Err(code.error()),
            _ => Err(ipc::protocol_error()),
        }
    }
    pub fn commit(&mut self) -> Result<(), AppError> {
        self.resolve(Command::Commit)
    }
    pub fn rollback(&mut self) -> Result<(), AppError> {
        self.resolve(Command::Rollback)
    }
    fn resolve(&mut self, command: Command) -> Result<(), AppError> {
        self.channel.send(&command)?;
        match self.channel.receive()? {
            Response::Complete => self.process.wait(Duration::from_secs(10)),
            Response::Failed { code } => Err(code.error()),
            _ => Err(ipc::protocol_error()),
        }
    }
}

pub fn entry() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_none_or(|argument| argument != "--rela-core-update")
    {
        return None;
    }
    Some(if run(&args).is_ok() { 0 } else { 1 })
}
fn run(args: &[OsString]) -> Result<(), AppError> {
    if args.len() != 2 || !platform::is_elevated() {
        return Err(ipc::protocol_error());
    }
    let executable = std::env::current_exe().map_err(|_| ipc::protocol_error())?;
    let candidate = executable.parent().ok_or_else(ipc::protocol_error)?;
    if candidate.file_name().is_none_or(|name| name != "candidate")
        || executable
            .file_name()
            .is_none_or(|name| !name.to_string_lossy().eq_ignore_ascii_case("Rela.exe"))
    {
        return Err(ipc::protocol_error());
    }
    let control = candidate.parent().ok_or_else(ipc::protocol_error)?;
    let path = control.join(REQUEST_FILE);
    package::plain_file(&path)?;
    if PathBuf::from(&args[1])
        .canonicalize()
        .map_err(|_| ipc::protocol_error())?
        != path.canonicalize().map_err(|_| ipc::protocol_error())?
    {
        return Err(ipc::protocol_error());
    }
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .map_err(|_| ipc::protocol_error())?
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| ipc::protocol_error())?;
    if bytes.len() > 16384 {
        return Err(ipc::protocol_error());
    }
    let request: Request =
        serde_json::from_slice(&platform::unprotect(&bytes)?).map_err(|_| ipc::protocol_error())?;
    request.endpoint.validate()?;
    if request.endpoint.role != Role::Core {
        return Err(ipc::protocol_error());
    }
    let parent = ProcessFence::open(&request.controller, &control.join("runner.exe"))?;
    if parent.has_exited()? {
        return Err(ipc::protocol_error());
    }
    let mut channel = Channel::connect(&request.endpoint)?;
    // Acquire the protected lock only after both endpoints authenticate one another.
    let mut participant = match CoreUpdateParticipant::resume(&request.endpoint.id) {
        Ok(value) => value,
        Err(error) => {
            channel.send(&Response::Failed {
                code: Failure::from_error(&error),
            })?;
            return Err(error);
        }
    };
    loop {
        let command: Command = channel.receive()?;
        if parent.has_exited()? {
            return Err(ipc::protocol_error());
        }
        let result = match command {
            Command::Prepare => participant
                .prepare_update(&candidate.join("easytier"))
                .map(|changed_service| Response::Prepared { changed_service }),
            Command::Commit => participant.commit().map(|_| Response::Complete),
            Command::Rollback => participant.rollback().map(|_| Response::Complete),
        };
        match result {
            Ok(response) => {
                channel.send(&response)?;
                if matches!(response, Response::Complete) {
                    return Ok(());
                }
            }
            Err(error) => channel.send(&Response::Failed {
                code: Failure::from_error(&error),
            })?,
        }
    }
}
