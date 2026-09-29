//! Official NSIS updater adapter. The external source of truth is the signed
//! Rela software feed. Tauri reads only a one-use loopback translation of that
//! verified selection; it never follows a second, unsigned online version feed.
use crate::distribution::{manifest_error, package::VerifiedPackage, transport::Fetcher};
use rela_manifests::{verify_envelope, Channel, PublicKey, Purpose, SoftwareManifest};
use rela_protocol::AppError;
use semver::Version;
use std::{io::Read, path::Path, time::Duration};
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const TARGET: &str = "windows-x86_64-nsis";
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST: usize = 4096;
// The official installer API needs one in-memory allocation. Refuse oversized
// installer metadata before downloading; Portable ZIPs retain their own limit.
const MAX_INSTALLER: u64 = 512 * 1024 * 1024;

pub struct InstallerPlan {
    manifest: SoftwareManifest,
    current: Version,
    envelope_digest: String,
}

impl InstallerPlan {
    /// Reverify the exact confirmed release. A deserialized manifest or stale UI
    /// version alone cannot authorize an installer download.
    pub fn from_signed(
        envelope: &[u8],
        keys: &[PublicKey],
        channel: Channel,
        current: &Version,
        confirmed: &Version,
    ) -> Result<Self, AppError> {
        let payload = verify_envelope(envelope, Purpose::Software, keys).map_err(manifest_error)?;
        let manifest = SoftwareManifest::parse(&payload, channel).map_err(manifest_error)?;
        if &manifest.version != confirmed
            || !manifest.newer_than(current).map_err(manifest_error)?
            || manifest.installer.size > MAX_INSTALLER
            || chrono::DateTime::parse_from_rfc3339(&manifest.published_at).is_err()
        {
            return Err(invalid());
        }
        Ok(Self {
            manifest,
            current: current.clone(),
            envelope_digest: rela_manifests::sha256(envelope),
        })
    }

    pub fn manifest(&self) -> &SoftwareManifest {
        &self.manifest
    }

    /// Download through Rela's HTTPS allowlist and bounded streaming verifier.
    /// There is deliberately no use of Tauri's unbounded download() method.
    pub async fn download(
        &self,
        public_key: &str,
        directory: &Path,
        progress: impl FnMut(u64, u64),
    ) -> Result<VerifiedPackage, AppError> {
        Fetcher::for_packages()?
            .download_package(&self.manifest.installer, public_key, directory, progress)
            .await
    }

    /// Prepare the official updater without installing or showing a window.
    /// NSIS restart is disabled: the transaction controller owns candidate
    /// startup, configuration recovery, and the final Core commit decision.
    pub async fn prepare<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        installed_executable: &Path,
    ) -> Result<OfficialInstaller, AppError> {
        self.prepare_with_args(app, installed_executable, None)
            .await
    }

    /// Only the independent launcher may bind a verified transaction to NSIS.
    /// /D is a final unquoted absolute directory, as required by NSIS itself.
    pub async fn prepare_transaction<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        request_path: &Path,
    ) -> Result<OfficialInstaller, AppError> {
        let request = super::nsis_worker::read_request(request_path)?;
        if request.recovery.is_some()
            || request.previous_version != self.current
            || request.envelope_digest != self.envelope_digest
            || request.channel != self.manifest.channel
        {
            return Err(invalid());
        }
        let args = transaction_args(&request.root, request_path)?;
        self.prepare_with_args(app, &request.root.join("Rela.exe"), Some(args))
            .await
    }

    async fn prepare_with_args<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        installed_executable: &Path,
        args: Option<Vec<String>>,
    ) -> Result<OfficialInstaller, AppError> {
        if app.package_info().version != self.current {
            return Err(invalid());
        }
        let public_key = adapter_config(app)?;
        let body = serde_json::to_vec(&serde_json::json!({
            "version": self.manifest.version,
            "notes": self.manifest.notes,
            "pub_date": self.manifest.published_at,
            "platforms": {
                TARGET: {
                    "url": self.manifest.installer.url,
                    "signature": self.manifest.installer.signature,
                },
            },
        }))
        .map_err(|_| invalid())?;
        let bridge = Bridge::bind(body).await?;
        let expected = self.manifest.clone();
        let bound = args.is_some();
        let updater = app
            .updater_builder()
            .target(TARGET)
            .endpoints(vec![bridge.url.parse().map_err(|_| invalid())?])
            .map_err(|_| invalid())?
            .executable_path(installed_executable)
            .restart_after_install(false)
            .clear_installer_args()
            .installer_args(args.unwrap_or_default())
            .no_proxy()
            .timeout(BRIDGE_TIMEOUT)
            .configure_client(|client| client.redirect(updater_http::redirect::Policy::none()))
            .version_comparator(move |current, remote| {
                remote.version == expected.version
                    && current >= expected.minimum_app_version
                    && remote.version > current
                    && remote
                        .download_url(TARGET)
                        .is_ok_and(|url| url.as_str() == expected.installer.url)
                    && remote
                        .signature(TARGET)
                        .is_ok_and(|signature| signature == &expected.installer.signature)
            })
            .build()
            .map_err(|_| invalid())?;
        let server = tauri::async_runtime::spawn(async move { bridge.serve().await });
        let result = updater.check().await;
        if result.is_err() {
            server.abort();
            return Err(invalid());
        }
        server.await.map_err(|_| invalid())??;
        let update = result.map_err(|_| invalid())?.ok_or_else(invalid)?;
        self.validate_update(&update)?;
        Ok(OfficialInstaller {
            update,
            manifest: self.manifest.clone(),
            public_key,
            bound,
        })
    }

    fn validate_update(&self, update: &Update) -> Result<(), AppError> {
        if update.current_version != self.current.to_string()
            || update.version != self.manifest.version.to_string()
            || update.target != TARGET
            || update.download_url.as_str() != self.manifest.installer.url
            || update.signature != self.manifest.installer.signature
            || update.body.as_deref() != Some(self.manifest.notes.as_str())
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Carries no unverified or caller-writable Tauri Update fields. No frontend
/// capability or application command exposes this adapter directly.
pub struct OfficialInstaller {
    update: Update,
    manifest: SoftwareManifest,
    public_key: String,
    bound: bool,
}

impl OfficialInstaller {
    /// Reopens and verifies the staged artifact while denying writes/deletion.
    /// Only a future installed-transaction controller should call launch();
    /// preparation itself never executes an installer.
    pub fn verify(self, path: &Path) -> Result<VerifiedInstaller, AppError> {
        let mut package = VerifiedPackage::open_versioned(
            path,
            &self.manifest.installer,
            &self.public_key,
            &self.manifest.version,
        )?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.manifest.installer.size as usize)
            .map_err(|_| invalid())?;
        package
            .reader()?
            .take(self.manifest.installer.size + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        if bytes.len() as u64 != self.manifest.installer.size || !bytes.starts_with(b"MZ") {
            return Err(invalid());
        }
        Ok(VerifiedInstaller {
            update: self.update,
            bytes,
            _package: package,
            bound: self.bound,
        })
    }
}

pub struct VerifiedInstaller {
    update: Update,
    bytes: Vec<u8>,
    _package: VerifiedPackage,
    bound: bool,
}

impl VerifiedInstaller {
    /// Starts the official NSIS path and exits this calling process on success.
    /// The caller must already have persisted a recovery transaction and an
    /// independent controller. UAC cancellation returns an error and must not
    /// commit Core. This API is not wired to the GUI until that exists.
    pub fn launch(self) -> Result<(), AppError> {
        if !self.bound {
            return Err(invalid());
        }
        self.update.install(&self.bytes).map_err(|_| {
            AppError::new(
                "installer_launch_failed",
                "未能启动安装程序，可能已取消管理员授权。请重试。",
            )
        })
    }
}

fn transaction_args(root: &Path, request: &Path) -> Result<Vec<String>, AppError> {
    fn local(path: &Path) -> Result<String, AppError> {
        let canonical = path.canonicalize().map_err(|_| invalid())?;
        let value = canonical.to_str().ok_or_else(invalid)?;
        let value = value.strip_prefix(r"\\?\").unwrap_or(value);
        let bytes = value.as_bytes();
        if bytes.len() < 4
            || !bytes[0].is_ascii_alphabetic()
            || &bytes[1..3] != b":\\"
            || value.chars().any(|c| c.is_control() || c == '"')
        {
            return Err(invalid());
        }
        Ok(value.to_owned())
    }
    let args = vec![
        format!("/RELAUPDATE=\"{}\"", local(request)?),
        format!("/D={}", local(root)?),
    ];
    // NSIS's 1024-character command buffer also contains its executable path.
    // Leave ample space for the official updater's temporary installer path.
    if args
        .iter()
        .map(|arg| arg.encode_utf16().count())
        .sum::<usize>()
        > 640
    {
        return Err(invalid());
    }
    Ok(args)
}

struct Bridge {
    listener: TcpListener,
    url: String,
    expected_line: Vec<u8>,
    body: Vec<u8>,
}

impl Bridge {
    async fn bind(body: Vec<u8>) -> Result<Self, AppError> {
        if body.len() > rela_manifests::MAX_PAYLOAD_BYTES {
            return Err(invalid());
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| invalid())?;
        let token = super::ipc::random_id()?;
        let path = format!("/{token}");
        let url = format!(
            "http://{}{}",
            listener.local_addr().map_err(|_| invalid())?,
            path
        );
        Ok(Self {
            listener,
            url,
            expected_line: format!("GET {path} HTTP/1.1\r\n").into_bytes(),
            body,
        })
    }

    async fn serve(self) -> Result<(), AppError> {
        tokio::time::timeout(BRIDGE_TIMEOUT, async {
            let (mut stream, peer) = self.listener.accept().await.map_err(|_| invalid())?;
            if !peer.ip().is_loopback() {
                return Err(invalid());
            }
            let mut request = Vec::new();
            let mut chunk = [0; 512];
            loop {
                let n = stream.read(&mut chunk).await.map_err(|_| invalid())?;
                if n == 0 || request.len() + n > MAX_REQUEST {
                    return Err(invalid());
                }
                request.extend_from_slice(&chunk[..n]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            if !request.starts_with(&self.expected_line) {
                return Err(invalid());
            }
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                self.body.len()
            );
            stream.write_all(header.as_bytes()).await.map_err(|_| invalid())?;
            stream.write_all(&self.body).await.map_err(|_| invalid())?;
            stream.shutdown().await.map_err(|_| invalid())
        }).await.map_err(|_| invalid())?
    }
}

fn invalid() -> AppError {
    AppError::new(
        "installer_update_invalid",
        "安装版更新信息不匹配或不受支持，请重新检查更新。",
    )
}

fn adapter_config<R: Runtime>(app: &AppHandle<R>) -> Result<String, AppError> {
    let value = app.config().plugins.0.get("updater").ok_or_else(invalid)?;
    let config: tauri_plugin_updater::Config =
        serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    if !config.endpoints.is_empty()
        || !config.dangerous_insecure_transport_protocol
        || config.dangerous_accept_invalid_certs
        || config.dangerous_accept_invalid_hostnames
        || config.allow_downgrades
        || !config.require_signed_version
        || config.windows.as_ref().is_none_or(|windows| {
            !windows.installer_args.is_empty() || windows.install_mode.to_string() != "passive"
        })
    {
        return Err(invalid());
    }
    crate::distribution::package::validate_public_key(&config.pubkey)?;
    Ok(config.pubkey)
}

#[cfg(test)]
mod tests;
