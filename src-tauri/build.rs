fn main() {
    use std::{env, fs, path::PathBuf};

    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let default_path = root.join("config/network.default.json");
    let local_path = root.join("config/network.local.json");
    println!("cargo:rerun-if-changed={}", default_path.display());
    println!("cargo:rerun-if-changed={}", local_path.display());
    println!("cargo:rerun-if-env-changed=RELA_NETWORK_SECRET");
    let selected = if local_path.exists() {
        local_path
    } else {
        default_path
    };
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(selected).expect("无法读取构建网络配置"))
            .expect("构建网络配置必须是有效的 JSON");
    if let Ok(secret) = env::var("RELA_NETWORK_SECRET") {
        config["network_secret"] = secret.into();
    }
    if env::var("PROFILE").as_deref() == Ok("release")
        && config["network_secret"].as_str().is_none_or(str::is_empty)
    {
        panic!("发布构建需要设置 RELA_NETWORK_SECRET 或 config/network.local.json");
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("default-network.json");
    fs::write(output, serde_json::to_vec(&config).unwrap()).expect("无法生成构建网络配置");
    tauri_build::build()
}
