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
    core.control(CoreAction::Connect, get_preferences(app)?.device_name)
        .await
}

#[tauri::command]
pub async fn disconnect(core: State<'_, Arc<EasyTierCore>>) -> Result<ConnectionStatus, AppError> {
    // 损坏的偏好文件不能阻止停止网络服务。
    core.control(CoreAction::Disconnect, Preferences::default().device_name)
        .await
}

#[tauri::command]
pub async fn reconnect(
    app: AppHandle,
    core: State<'_, Arc<EasyTierCore>>,
) -> Result<ConnectionStatus, AppError> {
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
    id: String,
    core: State<'_, Arc<EasyTierCore>>,
    resources: State<'_, Arc<ResourceManager>>,
) -> Result<(), AppError> {
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
    resources: State<'_, Arc<ResourceManager>>,
) -> Result<ResourceSyncStatus, AppError> {
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
    software: State<'_, Arc<SoftwareManager>>,
) -> Result<SoftwareUpdateStatus, AppError> {
    software.check().await
}

#[tauri::command]
pub fn set_update_channel(
    software: State<'_, Arc<SoftwareManager>>,
    channel: UpdateChannel,
) -> Result<SoftwareUpdateStatus, AppError> {
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
    core: State<'_, Arc<EasyTierCore>>,
    config: NetworkConfigUpdate,
) -> Result<NetworkConfigView, AppError> {
    core.save_configuration(config).await
}

#[tauri::command]
pub async fn reset_network_config(
    core: State<'_, Arc<EasyTierCore>>,
    resources: State<'_, Arc<ResourceManager>>,
) -> Result<NetworkConfigView, AppError> {
    let view = core.reset_configuration().await?;
    let resources = Arc::clone(resources.inner());
    tauri::async_runtime::spawn(async move {
        let _ = resources.refresh(false).await;
    });
    Ok(view)
}

/// Only the currently verified version can be opened; the UI supplies no URL.
#[tauri::command]
pub fn open_software_release(
    software: State<'_, Arc<SoftwareManager>>,
    version: String,
) -> Result<(), AppError> {
    let url = software.release_url(&version)?;
    platform::open_resource(&rela_manifests::Resource {
        id: "rela-release".into(),
        name: "Rela Release".into(),
        kind: rela_manifests::ResourceKind::Web,
        address: url,
        description: String::new(),
        port: None,
        username: None,
    })
    .map_err(|_| {
        AppError::new(
            "release_open_failed",
            "无法打开 GitHub Release 页面，请检查系统默认浏览器。",
        )
    })
}
