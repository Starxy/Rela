//! Native → Agent 的唯一边界。后续在此实现有 ACL 的 Windows Named Pipe 客户端。
//! 初始化阶段不启动高权限进程，也不以模拟状态替代真实 IPC 结果。
use rela_protocol::{AppError, ConnectionStatus, LabResource};

pub struct AgentClient;

impl AgentClient {
    pub fn get_status(&self) -> ConnectionStatus {
        ConnectionStatus::default()
    }

    pub fn connect(&self) -> Result<ConnectionStatus, AppError> {
        Err(AppError::agent_unavailable())
    }

    pub fn disconnect(&self) -> Result<ConnectionStatus, AppError> {
        Err(AppError::agent_unavailable())
    }

    pub fn reconnect(&self) -> Result<ConnectionStatus, AppError> {
        Err(AppError::agent_unavailable())
    }

    pub fn get_resources(&self) -> Vec<LabResource> {
        Vec::new()
    }
}
