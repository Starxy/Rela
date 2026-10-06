//! Routine service control uses only the configuration approved by an administrator.
use super::{
    deployment::{Deployment, PublicDeployment, Service},
    *,
};

#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Authorize,
    NoChange,
    Verify,
    WaitForStart,
    Start,
    Stop,
    Restart,
}

fn plan(
    action: CoreAction,
    state: ServiceState,
    approved: Option<&PublicDeployment>,
    expected: &Deployment,
    config_sha256: Option<&str>,
) -> Result<Plan, AppError> {
    if matches!(action, CoreAction::Disconnect) {
        return Ok(
            if matches!(state, ServiceState::Stopped | ServiceState::NotInstalled) {
                Plan::NoChange
            } else {
                Plan::Stop
            },
        );
    }
    let Some(approved) = approved else {
        return Ok(Plan::Authorize);
    };
    if approved.owner_app_version > expected.owner_app_version {
        return Err(AppError::new(
            "core_downgrade_blocked",
            "此 Rela 副本较旧，请使用部署当前引擎的较新版本。",
        ));
    }
    if approved.engine_revision != expected.assets.engine_revision
        || approved.core_version != expected.assets.version
        || approved.config_sha256.is_none()
        || approved.config_sha256.as_deref() != config_sha256
        || state == ServiceState::NotInstalled
    {
        return Ok(Plan::Authorize);
    }
    Ok(match (action, state) {
        (CoreAction::Reconnect, _) | (_, ServiceState::Stopping) => Plan::Restart,
        (_, ServiceState::Running) => Plan::Verify,
        (_, ServiceState::Starting) => Plan::WaitForStart,
        _ => Plan::Start,
    })
}

pub(super) fn try_control(
    request: &HelperRequest,
    bundled: &Path,
) -> Result<Option<bool>, AppError> {
    let root = platform::service_directory()?;
    // Refuse to operate on an unrelated service, even if the caller could control it.
    platform::owned_service_binary(&deployment::allowed_binaries(&root))?;
    let state = platform::service_state()?;
    let description = platform::service_description()?;
    let approved = description
        .as_deref()
        .and_then(PublicDeployment::from_description);
    let expected = Deployment::bundled()?;
    let config_sha256 = request
        .config
        .as_ref()
        .map(|config| {
            config
                .core_toml(&request.device_name)
                .map(|text| format!("{:x}", Sha256::digest(text.as_bytes())))
        })
        .transpose()?;
    let plan = plan(
        request.action,
        state,
        approved.as_ref(),
        &expected,
        config_sha256.as_deref(),
    )?;
    let (start, stop) = match plan {
        Plan::Authorize => return Ok(None),
        Plan::NoChange => return Ok(Some(false)),
        Plan::Start => (true, false),
        Plan::Stop => (false, true),
        Plan::Restart => (true, true),
        _ => (false, false),
    };
    if (start || stop) && !platform::service_control_allowed(start, stop)? {
        return Ok(None);
    }
    let mut service = managed_service::ManagedService::new(root, bundled.to_path_buf());
    if stop {
        service.stop()?;
    }
    if matches!(request.action, CoreAction::Disconnect) {
        return Ok(Some(false));
    }
    // A concurrent elevated deployment withdraws this approval before replacing files.
    if platform::service_description()? != description {
        return Err(AppError::new(
            "service_configuration_changed",
            "网络服务配置正在更换，请稍后重新连接。",
        ));
    }
    if start {
        service.start_and_verify(&expected.assets.version)?;
    } else {
        if plan == Plan::WaitForStart {
            wait_for(ServiceState::Running)?;
        }
        service.verify_running(&expected.assets.version)?;
    }
    Ok(Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_approved_config_can_connect_disconnect_and_reconnect_without_deployment() {
        let expected = Deployment::bundled().unwrap();
        let digest = "a".repeat(64);
        let mut approved = expected.public();
        approved.config_sha256 = Some(digest.clone());
        for (action, state, result) in [
            (CoreAction::Connect, ServiceState::Stopped, Plan::Start),
            (CoreAction::Connect, ServiceState::Running, Plan::Verify),
            (
                CoreAction::Connect,
                ServiceState::Starting,
                Plan::WaitForStart,
            ),
            (CoreAction::Reconnect, ServiceState::Running, Plan::Restart),
            (CoreAction::Disconnect, ServiceState::Running, Plan::Stop),
            (
                CoreAction::Disconnect,
                ServiceState::Stopped,
                Plan::NoChange,
            ),
            (
                CoreAction::Disconnect,
                ServiceState::NotInstalled,
                Plan::NoChange,
            ),
        ] {
            assert_eq!(
                plan(action, state, Some(&approved), &expected, Some(&digest)).unwrap(),
                result
            );
        }
    }

    #[test]
    fn missing_approval_configuration_changes_and_engine_changes_require_authorization() {
        let expected = Deployment::bundled().unwrap();
        let digest = "a".repeat(64);
        for case in 0..5 {
            let mut approved = expected.public();
            approved.config_sha256 = Some(digest.clone());
            match case {
                1 => approved.config_sha256 = None,
                2 => approved.config_sha256 = Some("b".repeat(64)),
                3 => approved.engine_revision += 1,
                4 => approved.core_version = "2.7.0-other".into(),
                _ => {}
            }
            assert_eq!(
                plan(
                    CoreAction::Connect,
                    ServiceState::Running,
                    (case != 0).then_some(&approved),
                    &expected,
                    Some(&digest)
                )
                .unwrap(),
                Plan::Authorize
            );
        }
        let mut approved = expected.public();
        approved.config_sha256 = Some(digest.clone());
        approved.owner_app_version = semver::Version::new(99, 0, 0);
        assert_eq!(
            plan(
                CoreAction::Connect,
                ServiceState::Stopped,
                Some(&approved),
                &expected,
                Some(&digest)
            )
            .unwrap_err()
            .code,
            "core_downgrade_blocked"
        );
        assert_eq!(
            plan(
                CoreAction::Disconnect,
                ServiceState::Running,
                Some(&approved),
                &expected,
                None
            )
            .unwrap(),
            Plan::Stop
        );
    }
}
