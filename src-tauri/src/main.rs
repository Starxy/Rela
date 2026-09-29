#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = rela_lib::easytier::helper_entry() {
        std::process::exit(code);
    }
    rela_lib::run()
}
