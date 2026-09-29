fn main() {
    // Network defaults come from the signed feed, never from developer files.
    // Only the distribution endpoints/public keys and generic local defaults are compiled.
    tauri_build::build();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Tauri attaches its resource/manifest to binaries only. Integration
        // tests constructing MockRuntime still link TaskDialogIndirect, which
        // needs Common Controls v6 before even reaching the test harness.
        let out = std::env::var("OUT_DIR").expect("Cargo OUT_DIR");
        println!("cargo:rustc-link-arg-tests={out}/resource.lib");
    }
}
