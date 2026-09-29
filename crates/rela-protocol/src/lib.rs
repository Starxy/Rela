//! Rela 前端与 Rust 后端之间的业务模型。不包含网络参数或设备凭据。
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 4;
pub const EASYTIER_TARGET_VERSION: &str = "2.7.0-0a783c8e";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoreState {
    Unavailable,
    Stopped,
    Starting,
    Stopping,
    Running,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GatewayState {
    Unknown,
    Online,
    Offline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionType {
    Direct,
    Relay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub connected: bool,
    pub core: CoreState,
    pub virtual_ip: Option<String>,
    pub gateway: GatewayState,
    pub latency_ms: Option<u32>,
    pub connection_type: Option<ConnectionType>,
    pub resources_available: u32,
    pub resources_total: u32,
    pub last_error: Option<String>,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self {
            connected: false,
            core: CoreState::Unavailable,
            virtual_ip: None,
            gateway: GatewayState::Unknown,
            latency_ms: None,
            connection_type: None,
            resources_available: 0,
            resources_total: 0,
            last_error: Some("网络引擎尚未接入。".into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Ssh,
    Web,
    Nas,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Unknown,
    Reachable,
    Unreachable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabResource {
    pub id: String,
    pub name: String,
    pub kind: ResourceKind,
    pub description: String,
    pub address: String,
    pub availability: Availability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckLevel {
    Pass,
    Warning,
    Error,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticCheck {
    pub id: String,
    pub label: String,
    pub level: CheckLevel,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub generated_at: String,
    pub summary: String,
    pub checks: Vec<DiagnosticCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub device_name: String,
    pub launch_at_login: bool,
    pub auto_connect: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            device_name: "我的电脑".into(),
            launch_at_login: false,
            auto_connect: false,
        }
    }
}

impl Preferences {
    pub fn validate(&self) -> Result<(), AppError> {
        let name = self.device_name.trim();
        if name.is_empty() || name.encode_utf16().count() > 64 || name.chars().any(char::is_control)
        {
            return Err(AppError::new(
                "invalid_preferences",
                "设备名称需为 1–64 个字符，且不能包含控制字符。",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub app: String,
    pub easytier_target: String,
    pub easytier_installed: Option<String>,
    pub protocol: u32,
}

/// 可供界面编辑的网络设置视图；已保存的密钥只返回是否存在。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfigView {
    pub network_name: String,
    pub has_credential: bool,
    pub peers: Vec<String>,
    pub private_mode: bool,
    pub disable_p2p: bool,
    pub gateway_ip: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfigUpdate {
    pub network_name: String,
    /// None 保留同网络的已存凭据；空字符串清除；非空值导入新凭据。
    pub credential_secret: Option<String>,
    pub peers: Vec<String>,
    pub private_mode: bool,
    pub disable_p2p: bool,
    pub gateway_ip: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
}

impl AppError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn core_unavailable() -> Self {
        Self::new("core_unavailable", "网络引擎尚未接入，暂时无法控制连接。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_core_never_claims_connectivity() {
        let status = ConnectionStatus::default();
        assert!(!status.connected);
        assert_eq!(status.core, CoreState::Unavailable);
        assert_eq!(status.gateway, GatewayState::Unknown);
        assert!(status.virtual_ip.is_none());
        assert_eq!(status.resources_available, 0);
    }

    #[test]
    fn core_wire_format_matches_frontend() {
        let status = serde_json::to_value(ConnectionStatus::default()).unwrap();
        assert_eq!(status["core"], "unavailable");
        assert!(status["virtual_ip"].is_null());
        assert_eq!(serde_json::to_value(CoreState::Stopped).unwrap(), "stopped");
        assert_eq!(serde_json::to_value(CoreState::Running).unwrap(), "running");
    }

    #[test]
    fn rejects_blank_and_overlong_device_names() {
        for name in [" ".to_string(), "x".repeat(65), "line\nbreak".to_string()] {
            let preferences = Preferences {
                device_name: name,
                ..Preferences::default()
            };
            assert!(preferences.validate().is_err());
        }
        assert!(Preferences::default().validate().is_ok());
    }
}
