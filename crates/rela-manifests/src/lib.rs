//! Public, signed distribution formats. No device preferences or credentials.
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fmt, net::IpAddr};
use url::Url;

pub const MAX_PAYLOAD_BYTES: usize = 512 * 1024;
pub const MAX_ENVELOPE_BYTES: usize = 768 * 1024;
pub const MAX_REVISION: u64 = 9_007_199_254_740_991;
pub const REPOSITORY: &str = "https://github.com/Starxy/Rela";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError(pub &'static str);
impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ManifestError {}
pub type Result<T> = std::result::Result<T, ManifestError>;
fn require(ok: bool, message: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(ManifestError(message))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Resources,
    Software,
}
impl Purpose {
    pub fn domain(self, key_id: &str) -> Vec<u8> {
        let purpose = match self {
            Self::Resources => "resources",
            Self::Software => "software",
        };
        format!("Rela signed manifest v1\n{purpose}\n{key_id}\n").into_bytes()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicKey {
    pub id: String,
    pub purpose: Purpose,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEnvelope {
    pub format: String,
    pub key_id: String,
    /// Original UTF-8 bytes encoded as Base64; no JSON canonicalization needed.
    pub payload: String,
    pub signature: String,
}

pub fn signing_message(purpose: Purpose, key_id: &str, payload: &[u8]) -> Result<Vec<u8>> {
    require(
        valid_id(key_id) && payload.len() <= MAX_PAYLOAD_BYTES,
        "签名输入无效。",
    )?;
    let mut message = purpose.domain(key_id);
    message.extend_from_slice(payload);
    Ok(message)
}

pub fn verify_envelope(bytes: &[u8], purpose: Purpose, keys: &[PublicKey]) -> Result<Vec<u8>> {
    require(bytes.len() <= MAX_ENVELOPE_BYTES, "清单超过大小限制。")?;
    let envelope: SignedEnvelope =
        serde_json::from_slice(bytes).map_err(|_| ManifestError("签名清单格式无效。"))?;
    require(
        envelope.format == "rela.signed.v1" && valid_id(&envelope.key_id),
        "签名格式不受支持。",
    )?;
    let matching: Vec<_> = keys
        .iter()
        .filter(|key| key.id == envelope.key_id && key.purpose == purpose)
        .collect();
    require(matching.len() == 1, "清单签名密钥不受信任。")?;
    let key: [u8; 32] = STANDARD
        .decode(&matching[0].public_key)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(ManifestError("验签公钥无效。"))?;
    let key = VerifyingKey::from_bytes(&key).map_err(|_| ManifestError("验签公钥无效。"))?;
    let payload = STANDARD
        .decode(envelope.payload)
        .map_err(|_| ManifestError("清单内容编码无效。"))?;
    let signature: [u8; 64] = STANDARD
        .decode(envelope.signature)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(ManifestError("清单签名无效。"))?;
    let message = signing_message(purpose, &envelope.key_id, &payload)?;
    key.verify_strict(&message, &Signature::from_bytes(&signature))
        .map_err(|_| ManifestError("清单签名验证失败。"))?;
    Ok(payload)
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Reusing a revision with different bytes is invalid, even with a valid signature.
pub fn check_revision(version: u64, digest: &str, previous: Option<(u64, &str)>) -> Result<()> {
    require((1..=MAX_REVISION).contains(&version), "清单版本号无效。")?;
    if let Some((old_version, old_digest)) = previous {
        require(version >= old_version, "清单版本低于已保存版本。")?;
        require(
            version != old_version || digest == old_digest,
            "相同版本的清单内容发生变化。",
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Ssh,
    Web,
    Nas,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub id: String,
    pub name: String,
    pub kind: ResourceKind,
    pub address: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkDefaults {
    pub network: String,
    pub peer: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceManifest {
    pub schema_version: u32,
    pub version: u64,
    pub network: String,
    pub peer: Vec<String>,
    pub resources: Vec<Resource>,
}

impl ResourceManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        require(bytes.len() <= MAX_PAYLOAD_BYTES, "清单超过大小限制。")?;
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| ManifestError("资源清单字段无效。"))?;
        value.validate()?;
        Ok(value)
    }

    pub fn defaults(&self) -> NetworkDefaults {
        NetworkDefaults {
            network: self.network.clone(),
            peer: self.peer.clone(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        require(self.schema_version == 1, "资源清单格式版本不受支持。")?;
        check_revision(self.version, "", None)?;
        validate_network(&self.network, &self.peer)?;
        require(self.resources.len() <= 256, "资源数量超过限制。")?;
        let mut ids = BTreeSet::new();
        for resource in &self.resources {
            resource.validate()?;
            require(ids.insert(&resource.id), "资源编号重复。")?;
        }
        Ok(())
    }
}

pub fn validate_network(network: &str, peers: &[String]) -> Result<()> {
    require(
        valid_text(network, 128) && network == network.trim(),
        "网络名称无效。",
    )?;
    require(!peers.is_empty() && peers.len() <= 16, "连接节点数量无效。")?;
    let mut unique = BTreeSet::new();
    for peer in peers {
        require(
            valid_text(peer, 2048) && unique.insert(peer),
            "连接节点无效或重复。",
        )?;
        let url = Url::parse(peer).map_err(|_| ManifestError("节点地址无效。"))?;
        let websocket = matches!(url.scheme(), "ws" | "wss");
        require(
            matches!(url.scheme(), "tcp" | "udp" | "quic" | "ws" | "wss")
                && url.host_str().is_some()
                && url.port() != Some(0)
                && (websocket || url.port().is_some())
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && (websocket || url.path().is_empty() || url.path() == "/")
                && peer == peer.trim(),
            "节点地址无效。",
        )?;
    }
    Ok(())
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= limit
        && !value.chars().any(char::is_control)
}
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
fn valid_host(value: &str) -> bool {
    if value.parse::<IpAddr>().is_ok() {
        return true;
    }
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}

impl Resource {
    pub fn validate(&self) -> Result<()> {
        require(
            valid_id(&self.id)
                && valid_text(&self.name, 128)
                && self.description.chars().count() <= 1024
                && !self.description.chars().any(char::is_control)
                && valid_text(&self.address, 2048),
            "资源字段无效。",
        )?;
        match self.kind {
            ResourceKind::Ssh => {
                require(
                    valid_host(&self.address) && self.port.is_some_and(|port| port != 0),
                    "SSH 主机或端口无效。",
                )?;
                if let Some(username) = &self.username {
                    require(
                        valid_id(username) && !username.starts_with('-'),
                        "SSH 用户名无效。",
                    )?;
                }
            }
            ResourceKind::Web => {
                let url = Url::parse(&self.address).map_err(|_| ManifestError("Web 地址无效。"))?;
                require(
                    matches!(url.scheme(), "http" | "https")
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.port() != Some(0)
                        && self.username.is_none()
                        && self
                            .port
                            .is_none_or(|port| Some(port) == url.port_or_known_default()),
                    "Web 地址或端口无效。",
                )?;
            }
            ResourceKind::Nas => {
                let parts: Vec<_> = self
                    .address
                    .strip_prefix("\\\\")
                    .unwrap_or("")
                    .split('\\')
                    .collect();
                require(
                    parts.len() >= 2
                        && valid_host(parts[0])
                        && self.username.is_none()
                        && self.port.is_none_or(|port| port == 445)
                        && parts[1..].iter().all(|part| {
                            !part.is_empty()
                                && *part != "."
                                && *part != ".."
                                && !part.ends_with(['.', ' '])
                                && !part
                                    .chars()
                                    .any(|c| c.is_control() || ":*?\"<>|/".contains(c))
                        }),
                    "NAS 路径无效。",
                )?;
            }
        }
        Ok(())
    }

    pub fn probe_target(&self) -> Result<(String, u16)> {
        self.validate()?;
        match self.kind {
            ResourceKind::Ssh => Ok((self.address.clone(), self.port.expect("validated port"))),
            ResourceKind::Web => {
                let url = Url::parse(&self.address).map_err(|_| ManifestError("Web 地址无效。"))?;
                // Host formatting must not leave brackets around IPv6 addresses for DNS resolution.
                let host = match url.host().expect("validated host") {
                    url::Host::Domain(host) => host.to_owned(),
                    url::Host::Ipv4(ip) => ip.to_string(),
                    url::Host::Ipv6(ip) => ip.to_string(),
                };
                Ok((host, url.port_or_known_default().expect("validated scheme")))
            }
            ResourceKind::Nas => Ok((
                self.address[2..]
                    .split('\\')
                    .next()
                    .expect("validated host")
                    .into(),
                445,
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Stable,
    Test,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub url: String,
    pub size: u64,
    pub sha256: String,
    /// Tauri/minisign artifact signature, independent of the manifest signature.
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoftwareManifest {
    pub schema_version: u32,
    pub revision: u64,
    pub version: Version,
    pub channel: Channel,
    pub target: String,
    pub notes: String,
    pub published_at: String,
    pub core_version: String,
    /// Minimum Rela version whose stored state can be upgraded automatically.
    pub minimum_app_version: Version,
    pub installer: Package,
    pub portable: Package,
}

impl SoftwareManifest {
    pub fn parse(bytes: &[u8], channel: Channel) -> Result<Self> {
        require(bytes.len() <= MAX_PAYLOAD_BYTES, "清单超过大小限制。")?;
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| ManifestError("软件清单字段无效。"))?;
        value.validate(channel)?;
        Ok(value)
    }

    pub fn validate(&self, channel: Channel) -> Result<()> {
        require(self.schema_version == 1, "软件清单格式版本不受支持。")?;
        check_revision(self.revision, "", None)?;
        require(
            self.channel == channel && (channel != Channel::Stable || self.version.pre.is_empty()),
            "软件渠道不匹配。",
        )?;
        require(self.target == "windows-x86_64", "软件架构不受支持。")?;
        require(
            self.minimum_app_version <= self.version && self.version.build.is_empty(),
            "软件版本范围无效。",
        )?;
        require(
            self.notes.len() <= 32768
                && valid_text(&self.published_at, 64)
                && valid_text(&self.core_version, 128),
            "软件说明或引擎版本无效。",
        )?;
        self.installer.validate(&self.version, ".exe")?;
        self.portable.validate(&self.version, ".zip")?;
        Ok(())
    }

    pub fn newer_than(&self, current: &Version) -> Result<bool> {
        require(
            current >= &self.minimum_app_version,
            "当前版本需要先手动升级。",
        )?;
        Ok(&self.version > current)
    }
}

impl Package {
    fn validate(&self, version: &Version, extension: &str) -> Result<()> {
        require(
            (1..=2 * 1024 * 1024 * 1024).contains(&self.size)
                && self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                && !self.signature.is_empty()
                && self.signature.len() <= 4096
                && STANDARD.decode(&self.signature).is_ok(),
            "软件包校验信息无效。",
        )?;
        let url = Url::parse(&self.url).map_err(|_| ManifestError("软件包地址无效。"))?;
        let prefix = format!("{REPOSITORY}/releases/download/v{version}/");
        let file = self.url.strip_prefix(&prefix).unwrap_or("");
        require(
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && !file.is_empty()
                && !file.contains(['/', '\\', '%'])
                && file.ends_with(extension)
                && file
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c)),
            "软件包必须固定到本仓库的版本 Release。",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
