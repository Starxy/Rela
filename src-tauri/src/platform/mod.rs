//! Windows 系统集成。普通启停使用已授权的服务权限；部署与配置变更使用一次性提权入口。
use rela_protocol::AppError;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub const SERVICE_NAME: &str = "RelaEasyTier";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    NotInstalled,
    Stopped,
    Starting,
    Stopping,
    Running,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;
#[cfg(not(windows))]
mod unsupported;
#[cfg(not(windows))]
pub use unsupported::*;

pub fn storage_error() -> AppError {
    AppError::new(
        "storage_unavailable",
        "无法保存本地配置，请检查磁盘和目录权限。",
    )
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let parent = path.parent().ok_or_else(storage_error)?;
    fs::create_dir_all(parent).map_err(|_| storage_error())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| storage_error())?
        .as_nanos();
    let temp = parent.join(format!(".rela-{}-{stamp}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| storage_error())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| storage_error())?;
        drop(file);
        replace_file(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
