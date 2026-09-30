use super::ServiceState;
use rela_protocol::AppError;
use std::{
    net::Ipv4Addr,
    path::{Path, PathBuf},
};
fn unsupported() -> AppError {
    AppError::new("platform_unsupported", "当前网络连接支持 Windows x64。")
}
pub fn protect(_: &[u8]) -> Result<Vec<u8>, AppError> {
    Err(unsupported())
}
pub fn protect_request(_: &[u8]) -> Result<Vec<u8>, AppError> {
    Err(unsupported())
}
pub fn unprotect(_: &[u8]) -> Result<Vec<u8>, AppError> {
    Err(unsupported())
}
pub fn replace_file(from: &Path, to: &Path) -> Result<(), AppError> {
    std::fs::rename(from, to).map_err(|_| unsupported())
}
pub fn private_file(_: &Path) -> Result<(), AppError> {
    Err(unsupported())
}
pub fn service_directory() -> Result<PathBuf, AppError> {
    Err(unsupported())
}
pub fn secure_service_directory(_: &Path) -> Result<(), AppError> {
    Err(unsupported())
}
pub fn secure_service_file(_: &Path) -> Result<(), AppError> {
    Err(unsupported())
}
pub fn is_elevated() -> bool {
    false
}
pub fn lock_service_control(_: &Path) -> Result<std::fs::File, AppError> {
    Err(unsupported())
}
pub fn service_state() -> Result<ServiceState, AppError> {
    Err(unsupported())
}
pub fn stop_service() -> Result<(), AppError> {
    Err(unsupported())
}
pub fn start_service() -> Result<(), AppError> {
    Err(unsupported())
}
pub fn owned_service_binary(_: &[PathBuf]) -> Result<Option<PathBuf>, AppError> {
    Err(unsupported())
}
pub fn delete_service() -> Result<(), AppError> {
    Err(unsupported())
}
pub fn service_description() -> Result<Option<String>, AppError> {
    Err(unsupported())
}
pub fn set_service_description(_: &str) -> Result<(), AppError> {
    Err(unsupported())
}
pub fn elevate_helper(_: &Path) -> Result<bool, AppError> {
    Err(unsupported())
}
pub fn open_resource(_: &rela_manifests::Resource) -> Result<(), AppError> {
    Err(unsupported())
}
pub fn tun_has_ip(_: &str, _: Ipv4Addr) -> bool {
    false
}
pub fn ping(_: Ipv4Addr) -> Option<u32> {
    None
}
