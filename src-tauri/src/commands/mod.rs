use crate::{
    diagnostics,
    easytier::{CoreAction, EasyTierCore},
    platform,
};
use rela_protocol::{
    AppError, ConnectionStatus, DiagnosticReport, LabResource, NetworkConfigUpdate,
    NetworkConfigView, Preferences, VersionInfo, EASYTIER_TARGET_VERSION, PROTOCOL_VERSION,
};
use std::{fs, io::ErrorKind, path::PathBuf, sync::Arc};
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub async fn get_status(core: State<'_, Arc<EasyTierCore>>) -> Result<ConnectionStatus, AppError> {
    core.get_status().await
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
pub fn get_resources() -> Vec<LabResource> {
    // 待用户配置资源目录后在本机进行探测。
    Vec::new()
}

#[tauri::command]
pub fn open_resource(id: String) -> Result<(), AppError> {
    // 后续通过本地资源目录中的 ID 解析目标，不接受任意命令。
    if id.trim().is_empty() {
        return Err(AppError::new("invalid_resource", "资源编号不能为空。"));
    }
    Err(AppError::new(
        "resource_unavailable",
        "尚未配置实验室资源。",
    ))
}

#[tauri::command]
pub async fn run_diagnostics(
    core: State<'_, Arc<EasyTierCore>>,
) -> Result<DiagnosticReport, AppError> {
    Ok(diagnostics::report(&core.get_status().await?))
}

#[tauri::command]
pub async fn get_version(core: State<'_, Arc<EasyTierCore>>) -> Result<VersionInfo, AppError> {
    Ok(VersionInfo {
        app: env!("CARGO_PKG_VERSION").into(),
        easytier_target: EASYTIER_TARGET_VERSION.into(),
        easytier_installed: core.installed_version().await,
        protocol: PROTOCOL_VERSION,
    })
}

fn preferences_path(app: &AppHandle) -> Result<PathBuf, AppError> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join("preferences.json"))
        .map_err(|_| AppError::new("storage_unavailable", "无法读取本地设置目录。"))
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
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|_| AppError::new("storage_unavailable", "无法读取诊断目录。"))?;
    fs::create_dir_all(&dir)
        .map_err(|_| AppError::new("storage_unavailable", "无法创建诊断目录。"))?;
    let path = dir.join(format!(
        "rela-diagnostics-{}.json",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
    ));
    let status = core.get_status().await?;
    let report = diagnostics::report(&status);
    let version = VersionInfo {
        app: env!("CARGO_PKG_VERSION").into(),
        easytier_target: EASYTIER_TARGET_VERSION.into(),
        easytier_installed: core.installed_version().await,
        protocol: PROTOCOL_VERSION,
    };
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
) -> Result<NetworkConfigView, AppError> {
    core.reset_configuration().await
}
