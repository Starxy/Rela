use rela_protocol::AppError;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

/// A marker beside the executable opts this copy into local storage.
pub struct AppPaths {
    pub config: PathBuf,
    pub logs: PathBuf,
    pub webview: Option<PathBuf>,
}

impl AppPaths {
    pub fn resolve(app: &AppHandle) -> Result<Self, AppError> {
        let executable = std::env::current_exe().map_err(|_| paths_error())?;
        if let Some(paths) = Self::portable(&executable) {
            return Ok(paths);
        }
        Ok(Self {
            config: app.path().app_config_dir().map_err(|_| paths_error())?,
            logs: app.path().app_log_dir().map_err(|_| paths_error())?,
            webview: None,
        })
    }

    fn portable(executable: &Path) -> Option<Self> {
        let directory = executable.parent()?;
        if !directory.join("portable.txt").is_file() {
            return None;
        }
        let data = directory.join("data");
        Some(Self {
            config: data.join("config"),
            logs: data.join("logs"),
            webview: Some(data.join("webview")),
        })
    }
}

fn paths_error() -> AppError {
    AppError::new("storage_unavailable", "无法读取应用数据目录。")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_storage_follows_executable_and_requires_marker() {
        let root = std::env::temp_dir().join(format!("rela-paths-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("中文 Portable");
        let moved = root.join("moved");
        std::fs::create_dir_all(&first).unwrap();
        assert!(AppPaths::portable(&first.join("Rela.exe")).is_none());
        std::fs::write(first.join("portable.txt"), "").unwrap();
        let paths = AppPaths::portable(&first.join("Rela.exe")).unwrap();
        assert_eq!(paths.config, first.join("data/config"));
        assert_eq!(paths.logs, first.join("data/logs"));
        assert_eq!(paths.webview, Some(first.join("data/webview")));
        std::fs::rename(&first, &moved).unwrap();
        assert_eq!(
            AppPaths::portable(&moved.join("Rela.exe")).unwrap().config,
            moved.join("data/config")
        );
        std::fs::remove_file(moved.join("portable.txt")).unwrap();
        std::fs::remove_dir(moved).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
