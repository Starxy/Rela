fn main() {
    use std::{env, fs, path::PathBuf};

    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let default_path = root.join("config/network.default.json");
    println!("cargo:rerun-if-changed={}", default_path.display());
    let defaults: serde_json::Value =
        serde_json::from_slice(&fs::read(default_path).expect("无法读取构建网络配置"))
            .expect("构建网络配置必须是有效的 JSON");
    // Only public defaults enter the binary. Local DPAPI files, local JSON and
    // RELA_NETWORK_SECRET are never build inputs, in debug or release builds.
    let config = serde_json::json!({
        "network_name": defaults["network_name"],
        "peers": defaults["peers"],
        "private_mode": defaults["private_mode"],
        "disable_p2p": defaults["disable_p2p"],
        "gateway_ip": defaults["gateway_ip"],
    });
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("default-network.json");
    fs::write(output, serde_json::to_vec(&config).unwrap()).expect("无法生成构建网络配置");
    tauri_build::build()
}
