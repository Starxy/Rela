//! The installed GUI holds a shared read-only file lock across Windows users.
//! PREINSTALL takes it exclusively until the protected pending journal exists;
//! subsequent starts then refuse the pending transaction before configuration IO.
use super::{native, pending};
use crate::distribution::package;
use rela_protocol::AppError;
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::Path,
};

pub const RUNNING_LOCK: &str = ".rela-installed-running.lock";
pub const CONTROL: &str = ".rela-installed-control";
pub struct Lease {
    _file: File,
}

impl Lease {
    pub fn shared(root: &Path) -> Result<Self, AppError> {
        let result = Self::acquire(root, false)?;
        if pending_update(root)? {
            return Err(pending());
        }
        Ok(result)
    }
    pub(in crate::updates) fn exclusive(root: &Path) -> Result<Self, AppError> {
        Self::acquire(root, true)
    }
    /// Only after installed_controller authenticates its gated candidate.
    pub(in crate::updates) fn candidate(root: &Path) -> Result<Self, AppError> {
        Self::acquire(root, false)
    }
    fn acquire(root: &Path, exclusive: bool) -> Result<Self, AppError> {
        let path = root.join(RUNNING_LOCK);
        package::plain_file(&path)?;
        native::trust_regular_file(&path)?;
        Self::lock_file(&path, exclusive)
    }
    fn lock_file(path: &Path, exclusive: bool) -> Result<Self, AppError> {
        // LockFileEx accepts GENERIC_READ. Ordinary users do not get write access
        // to the installation merely to participate in its process fence.
        let file = OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(path)
            .map_err(|_| pending())?;
        let locked = if exclusive {
            fs2::FileExt::try_lock_exclusive(&file)
        } else {
            fs2::FileExt::try_lock_shared(&file)
        };
        locked.map_err(|_| {
            AppError::new(
                "installed_copy_running",
                "此安装目录仍有 Rela 在运行或正在更新，请先退出该目录的其他窗口。",
            )
        })?;
        Ok(Self { _file: file })
    }
}
pub fn pending_update(root: &Path) -> Result<bool, AppError> {
    package::plain_directory(root)?;
    Ok(root.join(CONTROL).try_exists().map_err(|_| pending())?
        || root
            .join(super::DIRECTORY)
            .try_exists()
            .map_err(|_| pending())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn read_only_handles_share_and_exclude_installation_without_replacing_lock_file() {
        let root = std::env::temp_dir().join(format!(
            "rela-installed-lease-test-{}",
            crate::updates::ipc::random_id().unwrap()
        ));
        fs::create_dir(&root).unwrap();
        let path = root.join(RUNNING_LOCK);
        fs::write(&path, b"").unwrap();
        let one = Lease::lock_file(&path, false).unwrap();
        let two = Lease::lock_file(&path, false).unwrap();
        assert!(Lease::lock_file(&path, true).is_err());
        assert!(fs::remove_file(&path).is_err());
        drop(one);
        drop(two);
        let update = Lease::lock_file(&path, true).unwrap();
        assert!(Lease::lock_file(&path, false).is_err());
        assert!(Lease::lock_file(&path, true).is_err());
        drop(update);
        assert!(!pending_update(&root).unwrap());
        fs::create_dir(root.join(CONTROL)).unwrap();
        assert!(pending_update(&root).unwrap());
        fs::remove_dir(root.join(CONTROL)).unwrap();
        fs::remove_file(path).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
