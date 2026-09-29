use rela_manifests::{
    verify_envelope, Channel, PublicKey, Purpose, ResourceManifest, SoftwareManifest,
    MAX_ENVELOPE_BYTES, MAX_PAYLOAD_BYTES,
};
use serde::Deserialize;
use std::{fs::File, io::Read};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 && args.len() != 3 {
        return Err("用法：rela-manifest <resources|stable|test> <清单文件>；或 verify-resources/verify-stable/verify-test <签名清单> <公钥配置>".into());
    }
    let mut bytes = Vec::new();
    File::open(&args[1])?
        .take(MAX_ENVELOPE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    let kind = args[0].to_str().ok_or("清单类型无效。")?;
    let kind = if let Some(kind) = kind.strip_prefix("verify-") {
        if args.len() != 3 {
            return Err("缺少公钥配置文件。".into());
        }
        #[derive(Deserialize)]
        struct Keys {
            keys: Vec<PublicKey>,
        }
        let mut trusted = Vec::new();
        File::open(&args[2])?
            .take(64 * 1024 + 1)
            .read_to_end(&mut trusted)?;
        if trusted.len() > 64 * 1024 {
            return Err("公钥配置超过大小限制。".into());
        }
        let trusted: Keys = serde_json::from_slice(&trusted)?;
        bytes = verify_envelope(
            &bytes,
            if kind == "resources" {
                Purpose::Resources
            } else {
                Purpose::Software
            },
            &trusted.keys,
        )?;
        kind
    } else {
        if args.len() != 2 || bytes.len() > MAX_PAYLOAD_BYTES {
            return Err("清单参数或大小无效。".into());
        }
        kind
    };
    match Some(kind) {
        Some("resources") => {
            ResourceManifest::parse(&bytes)?;
        }
        Some("stable") => {
            SoftwareManifest::parse(&bytes, Channel::Stable)?;
        }
        Some("test") => {
            SoftwareManifest::parse(&bytes, Channel::Test)?;
        }
        _ => return Err("清单类型无效。".into()),
    }
    println!("清单校验通过。");
    Ok(())
}
