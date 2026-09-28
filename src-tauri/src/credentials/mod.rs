//! 凭据仅允许由 Rela 的 Rust 后端写入/读取系统安全存储，禁止回传到前端或诊断包。
//! Windows Credential Manager / DPAPI 实现属于设备注册阶段。
use rela_protocol::AppError;

pub trait CredentialStore {
    fn save_device_credential(&self, credential: &[u8]) -> Result<(), AppError>;
    fn delete_device_credential(&self) -> Result<(), AppError>;
}
