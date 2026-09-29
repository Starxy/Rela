use crate::{
    app_paths::AppPaths,
    diagnostics,
    distribution::{manager::ResourceManager, software::SoftwareManager},
    easytier::{CoreAction, EasyTierCore},
    platform,
};
use rela_protocol::{
    AppError, ConnectionStatus, DiagnosticReport, LabResource, NetworkConfigUpdate,
    NetworkConfigView, Preferences, ResourceSyncStatus, SoftwareUpdateStatus, UpdateChannel,
    VersionInfo,
};
use std::{fs, io::ErrorKind, path::PathBuf, sync::Arc};
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub async fn get_status(
    core: State<'_, Arc<EasyTierCore>>,
    resources: State<'_, Arc<ResourceManager>>,
) -> Result<ConnectionStatus, AppError> {
    let mut status = core.get_status().await?;
    if let Ok((available, total)) = resources.counts(status.connected) {
        status.resources_available = available;
        status.resources_total = total;
    }
    Ok(status)
}

#[tauri::command]
pub async fn connect(
    app: AppHandle,
    core: State<'_, Arc<EasyTierCore>>,
) -> Result<ConnectionStatus, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    core.control(CoreAction::Connect, get_preferences(app)?.device_name)
        .await
}

#[tauri::command]
pub async fn disconnect(
    app: AppHandle,
    core: State<'_, Arc<EasyTierCore>>,
) -> Result<ConnectionStatus, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    // 损坏的偏好文件不能阻止停止网络服务。
    core.control(CoreAction::Disconnect, Preferences::default().device_name)
        .await
}

#[tauri::command]
pub async fn reconnect(
    app: AppHandle,
    core: State<'_, Arc<EasyTierCore>>,
) -> Result<ConnectionStatus, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    core.control(CoreAction::Reconnect, get_preferences(app)?.device_name)
        .await
}

#[tauri::command]
pub async fn get_resources(
    core: State<'_, Arc<EasyTierCore>>,
    resources: State<'_, Arc<ResourceManager>>,
) -> Result<Vec<LabResource>, AppError> {
    resources
        .resources(core.get_status().await?.connected)
        .await
}

#[tauri::command]
pub async fn open_resource(
    app: AppHandle,
    id: String,
    core: State<'_, Arc<EasyTierCore>>,
    resources: State<'_, Arc<ResourceManager>>,
) -> Result<(), AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    if !core.get_status().await?.connected {
        return Err(AppError::new(
            "resource_unavailable",
            "请先连接实验室网络。",
        ));
    }
    let resources = Arc::clone(resources.inner());
    tauri::async_runtime::spawn_blocking(move || resources.open(&id))
        .await
        .map_err(|_| AppError::new("resource_open_failed", "资源打开未完成，请重试。"))?
}

#[tauri::command]
pub fn get_resource_sync(
    resources: State<'_, Arc<ResourceManager>>,
) -> Result<ResourceSyncStatus, AppError> {
    resources.status()
}

#[tauri::command]
pub async fn refresh_resources(
    app: AppHandle,
    resources: State<'_, Arc<ResourceManager>>,
) -> Result<ResourceSyncStatus, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    resources.refresh(true).await
}

#[tauri::command]
pub fn get_software_update(
    software: State<'_, Arc<SoftwareManager>>,
) -> Result<SoftwareUpdateStatus, AppError> {
    software.status()
}

#[tauri::command]
pub async fn check_software_update(
    app: AppHandle,
    software: State<'_, Arc<SoftwareManager>>,
) -> Result<SoftwareUpdateStatus, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    software.check().await
}

#[tauri::command]
pub fn set_update_channel(
    app: AppHandle,
    software: State<'_, Arc<SoftwareManager>>,
    channel: UpdateChannel,
) -> Result<SoftwareUpdateStatus, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    software.set_channel(channel)
}

#[tauri::command]
pub async fn run_diagnostics(
    core: State<'_, Arc<EasyTierCore>>,
) -> Result<DiagnosticReport, AppError> {
    Ok(diagnostics::report(&core.get_status().await?))
}

#[tauri::command]
pub async fn get_version(core: State<'_, Arc<EasyTierCore>>) -> Result<VersionInfo, AppError> {
    core.versions().await
}

fn preferences_path(app: &AppHandle) -> Result<PathBuf, AppError> {
    Ok(app.state::<AppPaths>().config.join("preferences.json"))
}

#[tauri::command]
pub fn get_preferences(app: AppHandle) -> Result<Preferences, AppError> {
    match fs::read(preferences_path(&app)?) {
        Ok(bytes) => {
            let preferences: Preferences = serde_json::from_slice(&bytes).map_err(|_| {
                AppError::new("invalid_preferences", "本地设置文件损坏，请联系管理员。")
            })?;
            preferences.validate()?;
            Ok(preferences)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Preferences::default()),
        Err(_) => Err(AppError::new("storage_unavailable", "无法读取本地设置。")),
    }
}

#[tauri::command]
pub fn save_preferences(
    app: AppHandle,
    mut preferences: Preferences,
) -> Result<Preferences, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    preferences.device_name = preferences.device_name.trim().into();
    preferences.validate()?;
    if preferences.auto_connect || preferences.launch_at_login {
        return Err(AppError::new(
            "not_supported",
            "开机启动和自动连接尚未实现。",
        ));
    }
    let path = preferences_path(&app)?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("storage_unavailable", "设置目录无效。"))?;
    fs::create_dir_all(parent)
        .map_err(|_| AppError::new("storage_unavailable", "无法创建设置目录。"))?;
    let data = serde_json::to_vec_pretty(&preferences)
        .map_err(|_| AppError::new("serialization_failed", "设置格式无效。"))?;
    platform::atomic_write(&path, &data)?;
    Ok(preferences)
}

#[tauri::command]
pub async fn export_logs(
    app: AppHandle,
    core: State<'_, Arc<EasyTierCore>>,
) -> Result<String, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    // 仅序列化白名单业务字段；尚未收集原始日志，避免未经脱敏的内容泄漏。
    let dir = app.state::<AppPaths>().logs.clone();
    fs::create_dir_all(&dir)
        .map_err(|_| AppError::new("storage_unavailable", "无法创建诊断目录。"))?;
    let path = dir.join(format!(
        "rela-diagnostics-{}.json",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
    ));
    let status = core.get_status().await?;
    let report = diagnostics::report(&status);
    let version = core.versions().await?;
    let data = serde_json::to_vec_pretty(
        &serde_json::json!({ "version": version, "report": report, "status": status }),
    )
    .map_err(|_| AppError::new("serialization_failed", "无法生成诊断摘要。"))?;
    fs::write(&path, data)
        .map_err(|_| AppError::new("storage_unavailable", "无法保存诊断摘要。"))?;
    Ok(format!("诊断摘要已保存至 {}", path.display()))
}

#[tauri::command]
pub async fn get_network_config(
    core: State<'_, Arc<EasyTierCore>>,
) -> Result<NetworkConfigView, AppError> {
    core.configuration().await
}

#[tauri::command]
pub async fn save_network_config(
    app: AppHandle,
    core: State<'_, Arc<EasyTierCore>>,
    config: NetworkConfigUpdate,
) -> Result<NetworkConfigView, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    core.save_configuration(config).await
}

#[tauri::command]
pub async fn reset_network_config(
    app: AppHandle,
    core: State<'_, Arc<EasyTierCore>>,
    resources: State<'_, Arc<ResourceManager>>,
) -> Result<NetworkConfigView, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _operation = gate.operation()?;
    let view = core.reset_configuration().await?;
    let resources = Arc::clone(resources.inner());
    tauri::async_runtime::spawn(async move {
        if let Ok(_permit) = gate.operation() {
            let _ = resources.refresh(false).await;
        }
    });
    Ok(view)
}

/// Called after React mounts. The candidate remains hidden until this succeeds.
#[tauri::command]
pub async fn complete_update_startup(app: AppHandle) -> Result<Option<String>, AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    if !gate.begin_completion() {
        #[cfg(windows)]
        {
            return Ok(app
                .state::<Arc<crate::updates::startup::Startup>>()
                .notice
                .clone());
        }
        #[cfg(not(windows))]
        {
            return Ok(None);
        }
    }
    #[cfg(windows)]
    {
        let startup = Arc::clone(app.state::<Arc<crate::updates::startup::Startup>>().inner());
        let resources = Arc::clone(app.state::<Arc<ResourceManager>>().inner());
        let software = Arc::clone(app.state::<Arc<SoftwareManager>>().inner());
        let check_app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            get_preferences(check_app)?;
            resources.store.configuration()?;
            resources.status()?;
            software.status()?;
            startup.complete()
        })
        .await
        .map_err(|_| AppError::new("update_startup_failed", "新版初始化未完成。"))
        .and_then(|value| value);
        if let Err(error) = result {
            gate.failed();
            app.exit(1);
            return Err(error);
        }
    }
    gate.completed();
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
    crate::start_background_refresh(&app);
    Ok(Some(format!(
        "已更新到 Rela {}。",
        env!("CARGO_PKG_VERSION")
    )))
}

#[tauri::command]
pub fn get_update_progress(app: AppHandle) -> Result<rela_protocol::UpdateProgress, AppError> {
    app.state::<Arc<crate::updates::manager::UpdateManager>>()
        .status()
}

#[tauri::command]
pub async fn install_software_update(app: AppHandle, version: String) -> Result<(), AppError> {
    let gate = Arc::clone(app.state::<Arc<crate::updates::gate::Gate>>().inner());
    let _exclusive = gate.begin_update()?;
    let manager = Arc::clone(
        app.state::<Arc<crate::updates::manager::UpdateManager>>()
            .inner(),
    );
    let software = Arc::clone(app.state::<Arc<SoftwareManager>>().inner());
    let result: Result<(), AppError> = async {
        let selected = software.select(&version)?;
        let installed = app.state::<AppPaths>().webview.is_none();
        let total = selected.manifest.portable.size
            + if installed {
                selected.manifest.installer.size
            } else {
                0
            };
        manager.begin(version, total)?;
        #[cfg(windows)]
        {
            if installed {
                let config = app.state::<AppPaths>().config.clone();
                crate::updates::installed_controller::start(selected, &config, |received, total| {
                    manager.downloaded(received, total)
                })
                .await
            } else {
                crate::updates::controller::start(selected, |received, total| {
                    manager.downloaded(received, total)
                })
                .await
            }
        }
        #[cfg(not(windows))]
        {
            let _ = selected;
            Err(AppError::new("not_supported", "此平台暂不支持自动更新。"))
        }
    }
    .await;
    match result {
        Ok(()) => {
            manager.restarting();
            app.exit(0);
            Ok(())
        }
        Err(error) => {
            manager.failed(&error);
            gate.update_failed();
            Err(error)
        }
    }
}
