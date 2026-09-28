pub mod agent;
pub mod commands;
pub mod credentials;
pub mod diagnostics;
pub mod easytier;
pub mod platform;

pub fn run() {
    tauri::Builder::default()
        .manage(agent::AgentClient)
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::connect,
            commands::disconnect,
            commands::reconnect,
            commands::get_resources,
            commands::open_resource,
            commands::run_diagnostics,
            commands::export_logs,
            commands::get_version,
            commands::get_preferences,
            commands::save_preferences,
        ])
        .run(tauri::generate_context!())
        .expect("Rela 桌面应用启动失败");
}
