//! Rela 的 Rust 后端在此直接管理 EasyTier Core 的生命周期与 RPC。
//! 当前仅提供接入入口；进程控制和 RPC 尚未实现，不返回模拟连接结果。
use rela_protocol::{AppError, ConnectionStatus};

pub use rela_protocol::EASYTIER_TARGET_VERSION;
pub const RPC_LISTEN_ADDRESS: &str = "127.0.0.1";

pub struct EasyTierCore;

impl EasyTierCore {
    pub fn get_status(&self) -> ConnectionStatus {
        ConnectionStatus::default()
    }

    pub fn connect(&self) -> Result<ConnectionStatus, AppError> {
        Err(AppError::core_unavailable())
    }

    pub fn disconnect(&self) -> Result<ConnectionStatus, AppError> {
        Err(AppError::core_unavailable())
    }

    pub fn reconnect(&self) -> Result<ConnectionStatus, AppError> {
        Err(AppError::core_unavailable())
    }
}
