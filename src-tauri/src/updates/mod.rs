//! Application-update coordination. No installation command is exposed until
//! file recovery and the Core transaction participant are both integrated.
#[cfg(windows)]
pub mod configuration;
#[cfg(windows)]
pub mod controller;
pub mod coordination;
#[cfg(windows)]
pub mod core_worker;
pub mod gate;
#[cfg(windows)]
pub mod installed;
#[cfg(windows)]
pub mod installed_controller;
#[cfg(windows)]
pub mod installer;
pub mod manager;
#[cfg(windows)]
pub mod nsis_worker;
#[cfg(windows)]
pub mod startup;

pub fn package_public_key() -> Result<String, rela_protocol::AppError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Record {
        schema_version: u32,
        public_key: String,
    }
    let record: Record = serde_json::from_str(include_str!("../../../config/package-signing.json"))
        .map_err(|_| crate::distribution::manifest_error("包签名公钥配置无效。"))?;
    if record.schema_version != 1 {
        return Err(crate::distribution::manifest_error("包签名配置版本无效。"));
    }
    crate::distribution::package::validate_public_key(&record.public_key)?;
    Ok(record.public_key)
}
#[cfg(windows)]
pub mod ipc;
pub mod portable;
#[cfg(windows)]
pub mod process;
