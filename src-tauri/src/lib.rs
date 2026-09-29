mod app_paths;
pub mod commands;
pub mod diagnostics;
pub mod distribution;
pub mod easytier;
pub mod network_config;
pub mod platform;
mod tray;
pub mod updates;

use std::sync::Arc;
use tauri::Manager;

pub(crate) fn start_background_refresh(app: &tauri::AppHandle) {
    let resources = Arc::clone(
        app.state::<Arc<distribution::manager::ResourceManager>>()
            .inner(),
    );
    let software = Arc::clone(
        app.state::<Arc<distribution::software::SoftwareManager>>()
            .inner(),
    );
    let gate = Arc::clone(app.state::<Arc<updates::gate::Gate>>().inner());
    let resource_gate = Arc::clone(&gate);
    tauri::async_runtime::spawn(async move {
        if let Ok(_permit) = resource_gate.operation() {
            let _ = resources.refresh(false).await;
        }
    });
    tauri::async_runtime::spawn(async move {
        if let Ok(_permit) = gate.operation() {
            let _ = software.check().await;
        }
    });
}

pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        .setup(|app| {
            let paths = app_paths::AppPaths::resolve(app.handle())
                .map_err(|error| std::io::Error::other(error.message))?;
            #[cfg(windows)]
            let gate = {
                let startup = Arc::new(
                    updates::startup::Startup::initialize(&paths.config).map_err(|error| {
                        platform::show_update_error(&error.message);
                        std::io::Error::other(error.message)
                    })?,
                );
                let gate = Arc::clone(&startup.gate);
                app.manage(startup);
                gate
            };
            #[cfg(not(windows))]
            let gate = Arc::new(updates::gate::Gate::new(false));
            let candidate_startup = !gate.is_open();
            app.manage(Arc::clone(&gate));
            app.manage(Arc::new(updates::manager::UpdateManager::default()));
            let resources = app.path().resource_dir()?;
            let distribution = distribution::DistributionConfig::bundled()
                .map_err(|error| std::io::Error::other(error.message))?;
            let resource_manager = Arc::new(
                distribution::manager::ResourceManager::new(
                    paths.config.clone(),
                    distribution.clone(),
                )
                .map_err(|error| std::io::Error::other(error.message))?,
            );
            app.manage(Arc::new(easytier::EasyTierCore::new(
                paths.config.clone(),
                resources,
                Arc::clone(&resource_manager.store),
            )));
            app.manage(Arc::clone(&resource_manager));
            let software = Arc::new(
                distribution::software::SoftwareManager::new(
                    paths.config.clone(),
                    distribution,
                    if paths.webview.is_some() {
                        rela_protocol::InstallKind::Portable
                    } else {
                        rela_protocol::InstallKind::Installer
                    },
                )
                .map_err(|error| std::io::Error::other(error.message))?,
            );
            app.manage(Arc::clone(&software));
            let mut window = tauri::WebviewWindowBuilder::from_config(
                app.handle(),
                &app.config().app.windows[0],
            )?;
            if candidate_startup {
                window = window.visible(false);
            }
            if let Some(directory) = &paths.webview {
                std::fs::create_dir_all(directory)?;
                window = window.data_directory(directory.clone());
            }
            app.manage(paths);
            window.build()?;
            tray::setup(app)?;
            if candidate_startup {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(240)).await;
                    if gate.is_starting() {
                        handle.exit(1);
                    }
                });
            } else {
                start_background_refresh(app.handle());
            }
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
            commands::get_resource_sync,
            commands::refresh_resources,
            commands::get_software_update,
            commands::check_software_update,
            commands::set_update_channel,
            commands::complete_update_startup,
            commands::get_update_progress,
            commands::install_software_update,
        ])
        .run(tauri::generate_context!())
        .expect("Rela 桌面应用启动失败");
}
