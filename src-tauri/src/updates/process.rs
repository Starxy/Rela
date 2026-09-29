//! Wait on the exact parent process, never just a reusable process number.
use rela_protocol::AppError;
use serde::{Deserialize, Serialize};
use std::{
    mem::zeroed,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, ERROR_INVALID_PARAMETER, FILETIME, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    },
    System::Threading::{
        GetProcessId, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
        WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, SYNCHRONIZATION_SYNCHRONIZE,
    },
};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProcessTicket {
    pub pid: u32,
    // Decimal string remains lossless if the helper protocol is inspected with JS.
    pub started_at: String,
}

pub struct ProcessFence(OwnedHandle);

impl ProcessFence {
    fn process(pid: u32, expected: &Path) -> Result<Self, AppError> {
        let (result, actual) = Self::observe(pid)?;
        let expected = expected.canonicalize().map_err(|_| invalid())?;
        let normalize = |value: &Path| {
            value
                .to_string_lossy()
                .strip_prefix(r"\\?\")
                .unwrap_or(&value.to_string_lossy())
                .to_ascii_lowercase()
        };
        if normalize(&actual) != normalize(&expected) {
            return Err(invalid());
        }
        Ok(result)
    }

    fn observe(pid: u32) -> Result<(Self, PathBuf), AppError> {
        if pid == 0 {
            return Err(invalid());
        }
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZATION_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return Err(invalid());
        }
        let result = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
            return Err(invalid());
        }
        let actual = String::from_utf16(&buffer[..length as usize]).map_err(|_| invalid())?;
        Ok((result, actual.into()))
    }
    fn started_at(&self) -> Result<String, AppError> {
        let mut created: FILETIME = unsafe { zeroed() };
        let mut exited: FILETIME = unsafe { zeroed() };
        let mut kernel: FILETIME = unsafe { zeroed() };
        let mut user: FILETIME = unsafe { zeroed() };
        if unsafe {
            GetProcessTimes(
                self.0.as_raw_handle(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        } == 0
        {
            return Err(invalid());
        }
        Ok(
            ((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
                .to_string(),
        )
    }
    pub fn capture(pid: u32, expected: &Path) -> Result<ProcessTicket, AppError> {
        let process = Self::process(pid, expected)?;
        Ok(ProcessTicket {
            pid,
            started_at: process.started_at()?,
        })
    }
    /// Discover an authenticated helper/installer's actual image path. The
    /// caller must still verify its signed bytes before accepting any action.
    pub fn inspect(ticket: &ProcessTicket) -> Result<(Self, PathBuf), AppError> {
        let (process, image) = Self::observe(ticket.pid)?;
        if process.started_at()? != ticket.started_at || process.has_exited()? {
            return Err(invalid());
        }
        Ok((process, image))
    }
    pub fn capture_image(pid: u32) -> Result<(Self, ProcessTicket, PathBuf), AppError> {
        let (process, image) = Self::observe(pid)?;
        let ticket = ProcessTicket {
            pid,
            started_at: process.started_at()?,
        };
        if process.has_exited()? {
            return Err(invalid());
        }
        Ok((process, ticket, image))
    }
    /// A permissions/inspection failure is not evidence that an installer has
    /// exited. Return None only for a missing, exited, or reused process id.
    pub fn probe(ticket: &ProcessTicket) -> Result<Option<Self>, AppError> {
        if ticket.pid == 0
            || ticket
                .started_at
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .is_none()
        {
            return Err(invalid());
        }
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZATION_SYNCHRONIZE,
                0,
                ticket.pid,
            )
        };
        if handle.is_null() {
            return if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
                Ok(None)
            } else {
                Err(invalid())
            };
        }
        let process = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        if process.started_at()? != ticket.started_at || process.has_exited()? {
            return Ok(None);
        }
        Ok(Some(process))
    }
    /// NSIS hook entries are valid only while their verified NSIS parent lives.
    pub fn is_child_of(&self, child_pid: u32, parent: &ProcessTicket) -> Result<(), AppError> {
        if unsafe { GetProcessId(self.0.as_raw_handle()) } != child_pid || self.has_exited()? {
            return Err(invalid());
        }
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(invalid());
        }
        let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
        let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut available = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
        while available {
            if entry.th32ProcessID == child_pid {
                let (parent_process, _) = Self::inspect(parent)?;
                if entry.th32ParentProcessID == parent.pid
                    && !self.has_exited()?
                    && !parent_process.has_exited()?
                {
                    return Ok(());
                }
                return Err(invalid());
            }
            available = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
        }
        Err(invalid())
    }
    /// Open before the parent exits, then retain this handle throughout the handoff.
    pub fn open(ticket: &ProcessTicket, expected: &Path) -> Result<Self, AppError> {
        let process = Self::process(ticket.pid, expected)?;
        if process.started_at()? != ticket.started_at {
            return Err(invalid());
        }
        Ok(process)
    }
    pub fn wait(&self, timeout: Duration) -> Result<(), AppError> {
        let milliseconds = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), milliseconds) } {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(AppError::new(
                "update_parent_running",
                "Rela 尚未退出，程序文件未替换。请退出其他窗口后重试。",
            )),
            _ => Err(invalid()),
        }
    }
    pub fn has_exited(&self) -> Result<bool, AppError> {
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(invalid()),
        }
    }
}

fn invalid() -> AppError {
    AppError::new(
        "update_process_invalid",
        "无法确认更新进程身份，程序文件未替换。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    #[test]
    #[ignore = "spawned only by the process identity test"]
    fn fixture_child() {
        std::thread::sleep(Duration::from_millis(350));
    }

    #[test]
    fn exact_process_handle_waits_for_exit_and_rejects_wrong_start_or_program() {
        let executable = std::env::current_exe().unwrap();
        let mut child = Command::new(&executable)
            .args([
                "--exact",
                "updates::process::tests::fixture_child",
                "--ignored",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let ticket = ProcessFence::capture(child.id(), &executable).unwrap();
        let process = ProcessFence::open(&ticket, &executable).unwrap();
        assert!(ProcessFence::probe(&ticket).unwrap().is_some());
        let (_, observed_image) = ProcessFence::inspect(&ticket).unwrap();
        assert_eq!(
            observed_image.canonicalize().unwrap(),
            executable.canonicalize().unwrap()
        );
        let parent = ProcessFence::capture(std::process::id(), &executable).unwrap();
        process.is_child_of(child.id(), &parent).unwrap();
        assert!(process.is_child_of(child.id(), &ticket).is_err());
        let mut wrong = ticket.clone();
        wrong.started_at = "1".into();
        assert!(ProcessFence::open(&wrong, &executable).is_err());
        assert!(ProcessFence::open(&ticket, Path::new(r"C:\unrelated\Rela.exe")).is_err());
        assert_eq!(
            process.wait(Duration::ZERO).unwrap_err().code,
            "update_parent_running"
        );
        process.wait(Duration::from_secs(3)).unwrap();
        assert!(child.wait().unwrap().success());
        process.wait(Duration::ZERO).unwrap();
        assert!(ProcessFence::probe(&ticket).unwrap().is_none());
    }
}
