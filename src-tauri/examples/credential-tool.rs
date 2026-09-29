//! Local development helper: use the same validation, DPAPI and TOML serializer
//! as Rela. Only file paths are arguments; credentials never reach stdout/argv.
use rela_lib::{
    network_config::{self, NetworkConfig},
    platform,
};
use rela_protocol::{AppError, NetworkConfigUpdate};
use serde::Deserialize;
use std::{fs, io::Write, path::PathBuf};

#[derive(Deserialize)]
struct CredentialRecord {
    network_name: String,
    peers: Vec<String>,
    credential_secret: String,
}

fn run() -> Result<(), AppError> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 3 || (args[0] != "import" && args[0] != "probe-toml") {
        return Err(AppError::new(
            "usage",
            "用法：credential-tool <import|probe-toml> <DPAPI 凭据文件> <输出文件>",
        ));
    }
    let input = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    let bytes = fs::read(input).map_err(|_| platform::storage_error())?;
    let plain = platform::unprotect(&bytes)?;
    let record: CredentialRecord = serde_json::from_slice(&plain)
        .map_err(|_| AppError::new("invalid_record", "无法读取本地凭据记录。"))?;
    let mut config = NetworkConfig::bundled()?.updated(NetworkConfigUpdate {
        network_name: record.network_name,
        credential_secret: Some(record.credential_secret),
        peers: record.peers,
        private_mode: true,
        disable_p2p: true,
        gateway_ip: None,
    })?;
    config.normalize_and_validate(true)?;
    if args[0] == "import" {
        network_config::save(&output, &config)?;
    } else {
        let text = config.core_toml("rela-toml-probe")?;
        let mut value: toml::Value = toml::from_str(&text)
            .map_err(|_| AppError::new("invalid_config", "无法生成验证配置。"))?;
        let table = value.as_table_mut().expect("serialized config is a table");
        table.insert("stun_servers".into(), toml::Value::Array(vec![]));
        table.insert("stun_servers_v6".into(), toml::Value::Array(vec![]));
        for (key, enabled) in [
            ("no_tun", true),
            ("enable_ipv6", false),
            ("disable_upnp", true),
            ("disable_udp_hole_punching", true),
            ("disable_tcp_hole_punching", true),
        ] {
            table
                .get_mut("flags")
                .and_then(toml::Value::as_table_mut)
                .expect("serialized flags are a table")
                .insert(key.into(), toml::Value::Boolean(enabled));
        }
        // Create a fresh file and restrict ACLs before writing plaintext.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|_| platform::storage_error())?;
        platform::private_file(&output)?;
        let text = toml::to_string(&value)
            .map_err(|_| AppError::new("invalid_config", "无法生成验证配置。"))?;
        file.write_all(text.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| platform::storage_error())?;
    }
    println!("凭据已验证并写入本地受保护文件。");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{}", error.message);
        std::process::exit(1);
    }
}
