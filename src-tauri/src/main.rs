#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    if let Some(code) = rela_lib::updates::installed_controller::entry() {
        std::process::exit(code);
    }
    #[cfg(windows)]
    if let Some(code) = rela_lib::updates::nsis_worker::entry() {
        std::process::exit(code);
    }
    #[cfg(windows)]
    if let Some(code) = rela_lib::updates::controller::entry() {
        std::process::exit(code);
    }
    #[cfg(windows)]
    if let Some(code) = rela_lib::updates::core_worker::entry() {
        std::process::exit(code);
    }
    if let Some(code) = rela_lib::easytier::helper_entry() {
        std::process::exit(code);
    }
    rela_lib::run()
}
