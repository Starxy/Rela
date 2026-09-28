use crate::{agent::AgentClient, diagnostics};
use rela_protocol::{
    AppError, ConnectionStatus, DiagnosticReport, LabResource, Preferences, VersionInfo,
    EASYTIER_TARGET_VERSION, PROTOCOL_VERSION,
};
use std::{fs, io::ErrorKind, path::PathBuf};
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub fn get_status(agent: State<'_, AgentClient>) -> ConnectionStatus {
    agent.get_status()
}

#[tauri::command]
pub fn connect(agent: State<'_, AgentClient>) -> Result<ConnectionStatus, AppError> {
    agent.connect()
}

#[tauri::command]
pub fn disconnect(agent: State<'_, AgentClient>) -> Result<ConnectionStatus, AppError> {
    agent.disconnect()
}

#[tauri::command]
pub fn reconnect(agent: State<'_, AgentClient>) -> Result<ConnectionStatus, AppError> {
    agent.reconnect()
}

#[tauri::command]
pub fn get_resources(agent: State<'_, AgentClient>) -> Vec<LabResource> {
    agent.get_resources()
}

#[tauri::command]
pub fn open_resource(id: String) -> Result<(), AppError> {
    // 后续只能通过管理员下发的资源 ID 解析目标，不能接受前端传入的任意 URL/命令。
    if id.trim().is_empty() {
        return Err(AppError::new("invalid_resource", "资源编号不能为空。"));
    }
    Err(AppError::new(
        "resource_unavailable",
        "尚未从实验室获取资源，请先完成设备接入。",
    ))
}

#[tauri::command]
pub fn run_diagnostics() -> DiagnosticReport {
    diagnostics::unavailable_report()
}

#[tauri::command]
pub fn get_version() -> VersionInfo {
    VersionInfo {
        app: env!("CARGO_PKG_VERSION").into(),
        easytier_target: EASYTIER_TARGET_VERSION.into(),
        easytier_installed: None,
        protocol: PROTOCOL_VERSION,
    }
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
            "后台服务尚未接入，暂不支持开机启动和自动连接。",
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
    fs::write(path, data).map_err(|_| AppError::new("storage_unavailable", "无法保存设置。"))?;
    Ok(preferences)
}

#[tauri::command]
pub fn export_logs(app: AppHandle) -> Result<String, AppError> {
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
    let data = serde_json::to_vec_pretty(&serde_json::json!({ "version": get_version(), "report": run_diagnostics(), "status": ConnectionStatus::default() }))
        .map_err(|_| AppError::new("serialization_failed", "无法生成诊断摘要。"))?;
    fs::write(&path, data)
        .map_err(|_| AppError::new("storage_unavailable", "无法保存诊断摘要。"))?;
    Ok(format!("诊断摘要已保存至 {}", path.display()))
}
