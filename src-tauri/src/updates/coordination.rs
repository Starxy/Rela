//! Per-user cross-copy update fence. The elevated Core participant additionally
//! holds ProgramData's service lock, which covers other Windows accounts.
use crate::platform;
use rela_protocol::AppError;
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

pub struct Permit {
    _file: File,
}
impl Permit {
    pub fn network_control() -> Result<Self, AppError> {
        Self::acquire(&platform::coordination_directory()?, false)
    }
    pub fn update() -> Result<Self, AppError> {
        Self::acquire(&platform::coordination_directory()?, true)
    }
    fn acquire(directory: &Path, exclusive: bool) -> Result<Self, AppError> {
        // Check every existing component before create_dir_all can follow it.
        for ancestor in directory.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            match fs::symlink_metadata(ancestor) {
                Ok(metadata) => {
                    if !metadata.is_dir() || redirected(&metadata) {
                        return Err(platform::storage_error());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(platform::storage_error()),
            }
        }
        fs::create_dir_all(directory).map_err(|_| platform::storage_error())?;
        let path = directory.join("update.lock");
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() || redirected(&metadata) => {
                return Err(platform::storage_error())
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(platform::storage_error())
            }
            _ => {}
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(3);
        }
        let file = options.open(path).map_err(|_| platform::storage_error())?;
        let result = if exclusive {
            fs2::FileExt::try_lock_exclusive(&file)
        } else {
            fs2::FileExt::try_lock_shared(&file)
        };
        result.map_err(|_| {
            AppError::new(
                "update_in_progress",
                "另一个 Rela 副本正在更新或操作网络，请稍后重试。",
            )
        })?;
        Ok(Self { _file: file })
    }
}
fn redirected(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    fn directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rela-coordination-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    fn cleanup(directory: &Path) {
        fs::remove_file(directory.join("update.lock")).unwrap();
        fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn shared_network_operations_exclude_updates_and_update_excludes_all_controls() {
        let directory = directory();
        let one = Permit::acquire(&directory, false).unwrap();
        let two = Permit::acquire(&directory, false).unwrap();
        assert!(Permit::acquire(&directory, true).is_err());
        drop(one);
        drop(two);
        let update = Permit::acquire(&directory, true).unwrap();
        assert!(Permit::acquire(&directory, false).is_err());
        assert!(Permit::acquire(&directory, true).is_err());
        // File locks can be owned/dropped by a different worker thread.
        std::thread::spawn(move || drop(update)).join().unwrap();
        drop(Permit::acquire(&directory, false).unwrap());
        cleanup(&directory);
    }
    #[test]
    #[ignore = "spawned only by the cross-process lock test"]
    fn fixture_child() {
        let directory =
            std::path::PathBuf::from(std::env::var_os("RELA_TEST_COORDINATION").unwrap());
        let _lock = Permit::acquire(&directory, true).unwrap();
        fs::write(directory.join("ready"), b"ready").unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        // No destructor: validates OS release on actual process exit.
        std::process::exit(0);
    }
    #[test]
    fn exited_helper_releases_the_cross_process_update_lock() {
        use std::process::{Command, Stdio};
        let directory = directory();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "updates::coordination::tests::fixture_child",
                "--ignored",
            ])
            .env("RELA_TEST_COORDINATION", &directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !directory.join("ready").is_file() && Instant::now() < deadline {
            assert!(child.try_wait().unwrap().is_none());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(directory.join("ready").is_file());
        assert!(Permit::acquire(&directory, false).is_err());
        assert!(child.wait().unwrap().success());
        drop(Permit::acquire(&directory, true).unwrap());
        fs::remove_file(directory.join("ready")).unwrap();
        cleanup(&directory);
    }
}
