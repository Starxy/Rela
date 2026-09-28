use rela_protocol::{CheckLevel, DiagnosticCheck, DiagnosticReport};

pub fn unavailable_report() -> DiagnosticReport {
    DiagnosticReport {
        generated_at: chrono::Utc::now().to_rfc3339(),
        summary: "网络引擎不可用".into(),
        checks: vec![
            DiagnosticCheck {
                id: "core".into(),
                label: "网络引擎".into(),
                level: CheckLevel::Error,
                message: "尚未接入".into(),
            },
            DiagnosticCheck {
                id: "gateway".into(),
                label: "校园网关".into(),
                level: CheckLevel::Skipped,
                message: "未检测".into(),
            },
            DiagnosticCheck {
                id: "resources".into(),
                label: "实验室资源".into(),
                level: CheckLevel::Skipped,
                message: "未检测".into(),
            },
        ],
    }
}
