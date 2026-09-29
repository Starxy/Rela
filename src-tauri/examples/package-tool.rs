//! Local minisign release signing. Only DPAPI-encrypted private records touch disk.
use base64::{engine::general_purpose::STANDARD, Engine};
use rela_lib::{distribution::package::VerifiedPackage, platform};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivateRecord {
    schema_version: u32,
    secret_box: String,
    public_key: String,
}
#[derive(Serialize)]
struct PublicRecord {
    schema_version: u32,
    public_key: String,
}
fn main() {
    if run().is_err() {
        eprintln!("包签名操作失败，请检查输入、权限和密钥文件。");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|value| value.to_str()) {
        Some("generate") if args.len() == 3 => {
            let private = Path::new(&args[1]);
            let parent = private.parent().ok_or("missing parent")?;
            fs::create_dir_all(parent)?;
            let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("missing repository")?.canonicalize()?;
            if parent.canonicalize()?.starts_with(repo) { return Err("private key must be outside repository".into()); }
            let pair = minisign::KeyPair::generate_encrypted_keypair(Some(String::new()))?;
            let public_key = STANDARD.encode(pair.pk.to_box()?.to_string());
            let record = PrivateRecord { schema_version: 1, secret_box: pair.sk.to_box(Some("Rela package signing"))?.into_string(), public_key: public_key.clone() };
            let encrypted = platform::protect(&serde_json::to_vec(&record)?).map_err(|_| "DPAPI failed")?;
            let mut file = OpenOptions::new().create_new(true).write(true).open(private)?;
            platform::private_file(private).map_err(|_| "ACL failed")?;
            file.write_all(&encrypted)?; file.sync_all()?;
            let mut public = OpenOptions::new().create_new(true).write(true).open(&args[2])?;
            public.write_all(&serde_json::to_vec_pretty(&PublicRecord { schema_version: 1, public_key })?)?;
            public.sync_all()?;
            println!("包公钥已保存；私钥已通过当前用户 DPAPI 加密保存。");
        }
        Some(operation @ ("sign" | "sign-version")) if args.len() == (if operation == "sign-version" { 5 } else { 4 }) => {
            let version = semver::Version::parse(if operation == "sign-version" {
                args[4].to_str().ok_or("invalid version")?
            } else { env!("CARGO_PKG_VERSION") })?;
            if !version.build.is_empty() { return Err("build metadata unsupported".into()); }
            let record = load(Path::new(&args[1]))?;
            let key = minisign::SecretKeyBox::from_string(&record.secret_box)?.into_secret_key(Some(String::new()))?;
            let public = minisign::PublicKey::from_secret_key(&key)?;
            if STANDARD.encode(public.to_box()?.to_string()) != record.public_key { return Err("key mismatch".into()); }
            let mut options = OpenOptions::new(); options.read(true);
            #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(1); }
            let artifact = Path::new(&args[2]);
            let mut file = options.open(artifact)?;
            let size = file.metadata()?.len();
            if !(1..=rela_lib::distribution::package::MAX_PACKAGE_SIZE).contains(&size) { return Err("size invalid".into()); }
            let mut hash = Sha256::new(); std::io::copy(&mut file, &mut hash)?;
            file.seek(SeekFrom::Start(0))?;
            let comment = format!("version:{version}");
            let signature = minisign::sign(Some(&public), &key, &mut file, Some(&comment), Some("Rela update artifact"))?;
            let package = rela_manifests::Package { url: "https://github.com/Starxy/Rela/releases/download/verification-only/artifact.zip".into(), size, sha256: format!("{:x}", hash.finalize()), signature: STANDARD.encode(signature.to_string()) };
            // Reopen with the production streaming verifier before emitting release metadata.
            drop(VerifiedPackage::open_versioned(artifact, &package, &record.public_key, &version).map_err(|_| "verification failed")?);
            let output = Path::new(&args[3]);
            platform::atomic_write(output, &serde_json::to_vec_pretty(&serde_json::json!({ "version": version, "size": package.size, "sha256": package.sha256, "signature": package.signature }))?).map_err(|_| "write failed")?;
            platform::atomic_write(&output.with_extension("sig"), package.signature.as_bytes()).map_err(|_| "write failed")?;
            println!("更新包签名与独立校验通过，已生成签名和摘要元数据。");
        }
        Some("bundle") if args.len() == 2 => bundle(Path::new(&args[1]))?,
        _ => return Err("usage: package-tool generate <private.dat> <public.json> | sign <private.dat> <artifact> <metadata.json> | sign-version <private.dat> <artifact> <metadata.json> <version> | bundle <private.dat>".into()),
    }
    Ok(())
}

/// Compile the installer from an already built release. Decrypted key material
/// is inherited only by the fixed, locally installed Tauri bundler process. It
/// never enters a command argument, plaintext file, or echoed build output.
fn bundle(private: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let record = load(private)?;
    if record.public_key
        != rela_lib::updates::package_public_key().map_err(|_| "public key invalid")?
    {
        return Err("wrong package signing key".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("repository missing")?;
    let mut command = std::process::Command::new("node");
    command
        .arg(root.join("node_modules/@tauri-apps/cli/tauri.js"))
        .args(["bundle", "--bundles", "nsis", "--ci"])
        .current_dir(root)
        .env(
            "TAURI_SIGNING_PRIVATE_KEY",
            STANDARD.encode(&record.secret_box),
        )
        .env("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "")
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output()?;
    if !output.status.success() {
        // Even unexpected child diagnostics must not reveal its environment.
        return Err("Tauri installer bundling failed".into());
    }
    let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
    let directory = root.join("target/release/bundle/nsis");
    let name = format!("Rela_{version}_x64-setup.exe");
    let artifact = directory.join(&name);
    let mut signature = String::new();
    File::open(directory.join(format!("{name}.sig")))?
        .take(4097)
        .read_to_string(&mut signature)?;
    if signature.len() > 4096 {
        return Err("signature too large".into());
    }
    let mut file = File::open(&artifact)?;
    let size = file.metadata()?.len();
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    let package = rela_manifests::Package {
        url: format!("https://github.com/Starxy/Rela/releases/download/v{version}/{name}"),
        size,
        sha256: format!("{:x}", hash.finalize()),
        signature: signature.trim().into(),
    };
    let _verified =
        VerifiedPackage::open_versioned(&artifact, &package, &record.public_key, &version)
            .map_err(|_| "official signature verification failed")?;
    platform::atomic_write(
        &directory.join(format!("{name}.sha256")),
        format!("{}  {name}\n", package.sha256).as_bytes(),
    )
    .map_err(|_| "digest write failed")?;
    platform::atomic_write(
        &directory.join(format!("{name}.metadata.json")),
        &serde_json::to_vec_pretty(&serde_json::json!({ "version": version, "package": package }))?,
    )
    .map_err(|_| "metadata write failed")?;
    println!("NSIS 安装包已生成，Tauri 签名、摘要和签名版本已通过独立复验。未执行或发布安装包。");
    Ok(())
}
fn load(path: &Path) -> Result<PrivateRecord, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(16385).read_to_end(&mut bytes)?;
    if bytes.len() > 16384 {
        return Err("key too large".into());
    }
    let record: PrivateRecord =
        serde_json::from_slice(&platform::unprotect(&bytes).map_err(|_| "DPAPI failed")?)?;
    if record.schema_version != 1 {
        return Err("key format invalid".into());
    }
    Ok(record)
}
