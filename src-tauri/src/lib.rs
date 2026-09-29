pub mod commands;
pub mod diagnostics;
pub mod easytier;
pub mod network_config;
pub mod platform;
mod tray;

use std::sync::Arc;
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let config = app.path().app_config_dir()?;
            let resources = app.path().resource_dir()?;
            app.manage(Arc::new(easytier::EasyTierCore::new(config, resources)));
            tray::setup(app)?;
            Ok(())
        })
        .on_window_event(tray::on_window_event)
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
            commands::get_network_config,
            commands::save_network_config,
            commands::reset_network_config,
        ])
        .run(tauri::generate_context!())
        .expect("Rela 桌面应用启动失败");
}
