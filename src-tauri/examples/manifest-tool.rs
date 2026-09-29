//! Local release signing helper. Private key bytes only enter DPAPI and memory.
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use rela_lib::platform;
use rela_manifests::{
    signing_message, Channel, PublicKey, Purpose, ResourceManifest, SignedEnvelope,
    SoftwareManifest, MAX_PAYLOAD_BYTES,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateRecord {
    id: String,
    purpose: Purpose,
    secret: [u8; 32],
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|arg| arg.to_str()) {
        Some("generate") if args.len() == 5 => {
            let purpose = match args[1].to_str() { Some("resources") => Purpose::Resources, Some("software") => Purpose::Software, _ => return Err("签名用途无效。".into()) };
            let id = args[2].to_str().ok_or("密钥编号无效。")?;
            signing_message(purpose, id, &[])?;
            let private_path = Path::new(&args[3]);
            let parent = private_path.parent().ok_or("私钥目录无效。")?;
            fs::create_dir_all(parent)?;
            let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().canonicalize()?;
            if parent.canonicalize()?.starts_with(repo) { return Err("私钥必须保存在仓库目录之外。".into()); }
            let mut secret = [0; 32];
            getrandom::getrandom(&mut secret).map_err(|_| "无法生成随机密钥。")?;
            let key = SigningKey::from_bytes(&secret);
            let public = PublicKey { id: id.into(), purpose, public_key: STANDARD.encode(key.verifying_key().to_bytes()) };
            let record = PrivateRecord { id: id.into(), purpose, secret };
            let encrypted = platform::protect(&serde_json::to_vec(&record)?).map_err(|_| "无法保护签名密钥。")?;
            let mut file = fs::OpenOptions::new().write(true).create_new(true).open(private_path)?;
            platform::private_file(private_path).map_err(|_| "无法保护私钥文件权限。")?;
            file.write_all(&encrypted)?;
            file.sync_all()?;
            let public_path = Path::new(&args[4]);
            fs::create_dir_all(public_path.parent().ok_or("公钥目录无效。")?)?;
            let mut file = fs::OpenOptions::new().write(true).create_new(true).open(public_path)?;
            file.write_all(&serde_json::to_vec_pretty(&public)?)?;
            println!("公钥已生成，私钥已通过当前用户 DPAPI 加密保存。");
        }
        Some("sign") if args.len() == 4 => {
            let encrypted = read(Path::new(&args[1]), 16 * 1024)?;
            let plain = platform::unprotect(&encrypted).map_err(|_| "无法读取当前用户的签名密钥。")?;
            let record: PrivateRecord = serde_json::from_slice(&plain).map_err(|_| "签名密钥格式无效。")?;
            let payload = read(Path::new(&args[2]), MAX_PAYLOAD_BYTES)?;
            match record.purpose {
                Purpose::Resources => { ResourceManifest::parse(&payload)?; }
                Purpose::Software => {
                    let manifest: SoftwareManifest = serde_json::from_slice(&payload).map_err(|_| "软件清单无效。")?;
                    manifest.validate(if manifest.channel == Channel::Stable { Channel::Stable } else { Channel::Test })?;
                }
            }
            let key = SigningKey::from_bytes(&record.secret);
            let signature = key.sign(&signing_message(record.purpose, &record.id, &payload)?);
            let envelope = SignedEnvelope { format: "rela.signed.v1".into(), key_id: record.id,
                payload: STANDARD.encode(payload), signature: STANDARD.encode(signature.to_bytes()) };
            let output = Path::new(&args[3]);
            platform::atomic_write(output, &serde_json::to_vec_pretty(&envelope)?).map_err(|_| "无法保存已签名清单。")?;
            println!("清单已校验并签名。");
        }
        _ => return Err("用法：manifest-tool generate <resources|software> <key-id> <私钥.dat> <公钥.json>；或 sign <私钥.dat> <输入.json> <输出.json>".into()),
    }
    Ok(())
}

fn read(path: &Path, limit: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err("文件超过大小限制。".into());
    }
    Ok(bytes)
}
