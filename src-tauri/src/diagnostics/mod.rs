use rela_protocol::{
    CheckLevel, ConnectionStatus, CoreState, DiagnosticCheck, DiagnosticReport, GatewayState,
};

pub fn report(status: &ConnectionStatus) -> DiagnosticReport {
    DiagnosticReport {
        generated_at: chrono::Utc::now().to_rfc3339(),
        summary: if status.connected {
            "网络已连接".into()
        } else {
            status.last_error.clone().unwrap_or_else(|| "未连接".into())
        },
        checks: vec![
            DiagnosticCheck {
                id: "core".into(),
                label: "网络引擎".into(),
                level: match status.core {
                    CoreState::Running => CheckLevel::Pass,
                    CoreState::Unavailable => CheckLevel::Error,
                    _ => CheckLevel::Warning,
                },
                message: match status.core {
                    CoreState::Running => "运行中",
                    CoreState::Starting => "正在启动",
                    CoreState::Stopping => "正在停止",
                    CoreState::Stopped => "已停止",
                    CoreState::Unavailable => "不可用",
                }
                .into(),
            },
            DiagnosticCheck {
                id: "network".into(),
                label: "网络连接".into(),
                level: if status.connected {
                    CheckLevel::Pass
                } else {
                    CheckLevel::Warning
                },
                message: if status.connected {
                    "节点连接和虚拟网卡已就绪".into()
                } else {
                    status.last_error.clone().unwrap_or_else(|| "未连接".into())
                },
            },
            DiagnosticCheck {
                id: "gateway".into(),
                label: "校园网关".into(),
                level: match status.gateway {
                    GatewayState::Online => CheckLevel::Pass,
                    GatewayState::Offline => CheckLevel::Warning,
                    GatewayState::Unknown => CheckLevel::Skipped,
                },
                message: match status.gateway {
                    GatewayState::Online => format!("{} ms", status.latency_ms.unwrap_or_default()),
                    GatewayState::Offline => "未响应 ICMP 探测".into(),
                    GatewayState::Unknown => "未连接或未设置网关地址".into(),
                },
            },
            DiagnosticCheck {
                id: "resources".into(),
                label: "实验室资源".into(),
                level: CheckLevel::Skipped,
                message: "尚未配置资源目录".into(),
            },
        ],
    }
}
