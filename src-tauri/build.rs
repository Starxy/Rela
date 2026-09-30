fn main() {
    // Network defaults come from the public resources.json feed, never from developer files.
    // Only the distribution endpoints/public keys and generic local defaults are compiled.
    tauri_build::build();
}
