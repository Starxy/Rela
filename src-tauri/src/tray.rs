//! 窗口隐藏与网络连接相互独立；退出界面也不停止 Core 服务。
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager, Window, WindowEvent,
};

const TRAY_ID: &str = "rela-tray";

pub fn setup(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let open = MenuItem::with_id(app, "tray-open", "打开 Rela", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "tray-quit", "退出 Rela", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit])?;
    let icon = app
        .default_window_icon()
        .ok_or_else(|| std::io::Error::other("缺少 Rela 托盘图标"))?
        .clone();

    // 只有托盘成功创建后才允许隐藏窗口，避免窗口无法找回。
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("Rela")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "tray-open" => restore_window(app),
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                restore_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn restore_window(app: &AppHandle) {
    if app
        .try_state::<std::sync::Arc<crate::updates::gate::Gate>>()
        .is_some_and(|gate| gate.is_starting())
    {
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        // 先取消最小化，再显示，避免刚显示的窗口再次被最小化事件隐藏。
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != "main" || window.app_handle().tray_by_id(TRAY_ID).is_none() {
        return;
    }
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            if window.hide().is_ok() {
                api.prevent_close();
            }
        }
        // Windows 最小化会触发 Resized；普通尺寸变化不隐藏窗口。
        WindowEvent::Resized(_) if window.is_minimized().unwrap_or(false) => {
            let _ = window.hide();
        }
        _ => {}
    }
}
