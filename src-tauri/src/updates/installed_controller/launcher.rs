//! A headless Tauri context hosts only the official updater. No application
//! stores, tray, WebView or background feed jobs are initialized here.
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Decision {
    Launch,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Response {
    Ready,
    Failed,
}

pub(super) fn run(control: &Path) -> Result<(), AppError> {
    let state = load(control)?;
    let runner = state.runner.as_ref().ok_or_else(error)?;
    let parent = ProcessFence::open(runner, &control.join("runner.exe"))?;
    if parent.has_exited()? || state.phase != Phase::Launching {
        return Err(error());
    }
    let (source, _installer, _runner) = verify(control, &state)?;
    let _stage = source.stage.lock()?;
    let endpoint = state.launcher_endpoint.as_ref().ok_or_else(error)?;
    let mut channel = Channel::connect(endpoint)?;
    channel.send(&Response::Ready)?;
    match channel.receive::<Decision>()? {
        Decision::Launch => {}
    }
    // The controller persists this exact process before sending authorization.
    let state = load(control)?;
    let myself = ProcessFence::capture(std::process::id(), &control.join("runner.exe"))?;
    if state.launcher.as_ref() != Some(&myself)
        || state.phase != Phase::Launching
        || parent.has_exited()?
    {
        return Err(error());
    }
    let result = (|| {
        let app = tauri::Builder::default()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(tauri::generate_context!())
            .map_err(|_| error())?;
        let envelope = super::super::portable::read(
            &control.join("software.json"),
            rela_manifests::MAX_ENVELOPE_BYTES as u64,
        )?;
        let distribution = DistributionConfig::bundled()?;
        let plan = InstallerPlan::from_signed(
            &envelope,
            &distribution.keys,
            state.request.channel,
            &state.request.previous_version,
            &source.manifest.version,
        )?;
        let installer = tauri::async_runtime::block_on(
            plan.prepare_transaction(app.handle(), &control.join(nsis_worker::REQUEST_FILE)),
        )?;
        if parent.has_exited()? {
            return Err(error());
        }
        installer.verify(&control.join("installer.exe"))?.launch()
    })();
    if result.is_err() {
        let _ = channel.send(&Response::Failed);
    }
    result
}
