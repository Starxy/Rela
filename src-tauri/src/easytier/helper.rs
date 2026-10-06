//! Only controlled errors cross the elevated helper boundary; CLI output never does.
use rela_protocol::AppError;

const ERRORS: &[(u32, &str, &str)] = &[
    (
        2,
        "core_integrity_failed",
        "网络引擎文件校验失败，请重新安装 Rela。",
    ),
    (
        3,
        "service_conflict",
        "网络服务或管理端口被占用，请先关闭冲突的客户端。",
    ),
    (
        4,
        "core_downgrade_blocked",
        "此 Rela 副本较旧，请使用部署当前引擎的较新版本。",
    ),
    (
        5,
        "core_recovery_required",
        "网络引擎切换尚未恢复。已保留备份，请重试或联系管理员，勿删除服务数据。",
    ),
    (
        6,
        "core_deployment_rolled_back",
        "网络引擎部署未完成，已恢复原引擎和配置。",
    ),
    (
        7,
        "credential_migration_required",
        "已保留旧引擎和配置，但未恢复旧密码连接。请填写 credential 后重新连接。",
    ),
    (
        8,
        "core_version_unknown",
        "现有引擎版本无法验证，请使用较新的 Rela 或联系管理员恢复。",
    ),
    (
        20,
        "service_registration_failed_rolled_back",
        "网络服务注册失败，已恢复原引擎和配置。",
    ),
    (
        21,
        "service_conflict_rolled_back",
        "网络服务或管理端口冲突，已恢复原引擎和配置。",
    ),
    (
        22,
        "core_start_failed_rolled_back",
        "网络引擎未通过实例和版本校验，已恢复原引擎和配置。",
    ),
    (
        23,
        "service_timeout_rolled_back",
        "网络服务未及时就绪，已恢复原引擎和配置。",
    ),
    (
        24,
        "storage_unavailable_rolled_back",
        "网络服务文件写入失败，已恢复原引擎和配置。",
    ),
    (
        25,
        "core_integrity_failed_rolled_back",
        "网络引擎校验失败，已恢复原引擎和配置。",
    ),
    (
        26,
        "service_unavailable_rolled_back",
        "网络服务启停失败，已恢复原引擎和配置。",
    ),
    (
        27,
        "core_command_failed_rolled_back",
        "网络引擎命令执行失败，已恢复原引擎和配置。",
    ),
    (
        28,
        "service_permissions_failed",
        "网络服务启停权限设置失败，请重新连接以完成系统授权。",
    ),
];

pub(super) fn rolled_back(cause: &str) -> AppError {
    let code = format!("{cause}_rolled_back");
    let (_, code, message) = ERRORS
        .iter()
        .find(|(_, value, _)| *value == code)
        .unwrap_or(&ERRORS[4]);
    AppError::new(code, message)
}

pub(super) fn exit_code(result: Result<bool, AppError>) -> i32 {
    match result {
        Ok(true) => 0,
        Ok(false) => 10,
        Err(error) => ERRORS
            .iter()
            .find(|(_, code, _)| *code == error.code)
            .map(|(code, _, _)| *code as i32)
            .unwrap_or(1),
    }
}

pub(crate) fn decode_exit(code: u32) -> Result<bool, AppError> {
    match code {
        0 => Ok(true),
        10 => Ok(false),
        _ => {
            let (_, code, message) = ERRORS
                .iter()
                .find(|(value, _, _)| *value == code)
                .copied()
                .unwrap_or((
                    1,
                    "service_action_failed",
                    "网络引擎服务操作失败，请检查系统权限并重试。",
                ));
            Err(AppError::new(code, message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_preserves_controlled_failure_causes_and_never_returns_arbitrary_output() {
        for &(number, code, message) in ERRORS {
            let status = exit_code(Err(AppError::new(code, "synthetic-private-cli-output")));
            assert_eq!(status, number as i32);
            let error = decode_exit(status as u32).unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(error.message, message);
        }
        assert!(decode_exit(exit_code(Ok(true)) as u32).unwrap());
        assert!(!decode_exit(exit_code(Ok(false)) as u32).unwrap());
        assert_eq!(
            rolled_back("service_registration_failed").code,
            "service_registration_failed_rolled_back"
        );
        assert_eq!(
            rolled_back("synthetic-private-cli-output").code,
            "core_deployment_rolled_back"
        );
    }
}
