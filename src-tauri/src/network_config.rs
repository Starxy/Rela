use crate::platform;
use base64::{engine::general_purpose::STANDARD, Engine};
use rela_protocol::{AppError, NetworkConfigUpdate, NetworkConfigView};
use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, net::Ipv4Addr, path::Path};
use url::Url;

pub const INSTANCE_NAME: &str = "rela";
pub const INSTANCE_ID: &str = "5f9e7c9b-747a-47e5-b62b-3c2b607c312e";
pub const RPC_PORTAL: &str = "127.0.0.1:35888";
pub const TUN_NAME: &str = "Rela";

// 密钥可序列化到受保护的本机配置，但不实现 Debug，不能出现在日志中。
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    pub network_name: String,
    #[serde(default)]
    pub credential_secret: String,
    pub peers: Vec<String>,
    pub private_mode: bool,
    pub disable_p2p: bool,
    pub gateway_ip: Option<String>,
}

fn invalid(message: &str) -> AppError {
    AppError::new("invalid_network_config", message)
}

impl NetworkConfig {
    pub fn bundled() -> Result<Self, AppError> {
        let mut config: Self = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/default-network.json"
        )))
        .map_err(|_| invalid("内置网络配置无效。"))?;
        config.normalize_and_validate(false)?;
        Ok(config)
    }

    pub fn view(&self) -> NetworkConfigView {
        NetworkConfigView {
            network_name: self.network_name.clone(),
            has_credential: !self.credential_secret.is_empty(),
            peers: self.peers.clone(),
            private_mode: self.private_mode,
            disable_p2p: self.disable_p2p,
            gateway_ip: self.gateway_ip.clone(),
        }
    }

    pub fn updated(mut self, update: NetworkConfigUpdate) -> Result<Self, AppError> {
        if self.network_name != update.network_name.trim()
            && !self.credential_secret.is_empty()
            && update.credential_secret.is_none()
        {
            return Err(invalid(
                "更换网络时请同时导入新网络的 credential，或先清除原凭据。",
            ));
        }
        self.network_name = update.network_name;
        if let Some(secret) = update.credential_secret {
            self.credential_secret = secret;
        }
        self.peers = update.peers;
        self.private_mode = update.private_mode;
        self.disable_p2p = update.disable_p2p;
        self.gateway_ip = update.gateway_ip;
        self.normalize_and_validate(false)?;
        Ok(self)
    }

    pub fn normalize_and_validate(&mut self, require_credential: bool) -> Result<(), AppError> {
        self.network_name = self.network_name.trim().to_owned();
        if self.network_name.is_empty()
            || self.network_name.chars().count() > 128
            || self.network_name.chars().any(char::is_control)
        {
            return Err(invalid("网络名称需为 1–128 个字符，不能包含控制字符。"));
        }
        self.credential_secret = self.credential_secret.trim().to_owned();
        if self.credential_secret.is_empty() {
            if require_credential {
                return Err(invalid("请先在设置中导入此网络的 credential。"));
            }
        } else {
            credential_key(&self.credential_secret)?;
        }
        if self.peers.is_empty() || self.peers.len() > 16 {
            return Err(invalid("请填写 1–16 个连接节点。"));
        }
        let mut normalized = Vec::new();
        for peer in &self.peers {
            let peer = peer.trim();
            if peer.len() > 2048 || peer.chars().any(char::is_control) {
                return Err(invalid("连接节点地址无效。"));
            }
            let url = Url::parse(peer)
                .map_err(|_| invalid("请填写完整节点地址，例如 tcp://服务器:11010。"))?;
            if !matches!(url.scheme(), "tcp" | "udp" | "ws" | "wss" | "quic")
                || url.host_str().is_none()
                || url.port() == Some(0)
                || (!matches!(url.scheme(), "ws" | "wss") && url.port().is_none())
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || (!matches!(url.scheme(), "ws" | "wss")
                    && !url.path().is_empty()
                    && url.path() != "/")
            {
                return Err(invalid("节点须为带主机和端口的 tcp、udp、quic 或 WebSocket 地址，不能包含账号、查询参数或片段。"));
            }
            if !normalized.iter().any(|entry| entry == peer) {
                normalized.push(peer.to_owned());
            }
        }
        self.peers = normalized;
        self.gateway_ip = self
            .gateway_ip
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if self
            .gateway_ip
            .as_deref()
            .is_some_and(|ip| ip.parse::<Ipv4Addr>().is_err())
        {
            return Err(invalid("校园网关请填写有效的 IPv4 地址。"));
        }
        Ok(())
    }

    pub fn core_toml(&self, device_name: &str) -> Result<String, AppError> {
        let key = credential_key(&self.credential_secret)?;
        let public_key = STANDARD.encode(x25519_dalek::x25519(
            key,
            x25519_dalek::X25519_BASEPOINT_BYTES,
        ));
        let value = serde_json::json!({
            "instance_name": INSTANCE_NAME,
            "instance_id": INSTANCE_ID,
            "hostname": device_name,
            "dhcp": true,
            "listeners": [],
            "network_identity": { "network_name": self.network_name },
            "secure_mode": {
                "enabled": true,
                "local_private_key": self.credential_secret,
                "local_public_key": public_key,
            },
            "flags": { "private_mode": self.private_mode, "disable_p2p": self.disable_p2p, "dev_name": TUN_NAME },
            "peer": self.peers.iter().map(|uri| serde_json::json!({ "uri": uri })).collect::<Vec<_>>(),
        });
        let config = toml::Value::try_from(value).map_err(|_| invalid("无法生成网络配置。"))?;
        toml::to_string(&config).map_err(|_| invalid("无法生成网络配置。"))
    }
}

fn credential_key(secret: &str) -> Result<[u8; 32], AppError> {
    STANDARD
        .decode(secret)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| invalid("credential 格式无效，请粘贴管理员签发的 Base64 凭据。"))
}

pub fn load(path: &Path) -> Result<NetworkConfig, AppError> {
    match fs::read(path) {
        Ok(bytes) => {
            if bytes.len() > 128 * 1024 {
                return Err(invalid("本地网络配置损坏。"));
            }
            let plain = platform::unprotect(&bytes)?;
            let mut value: serde_json::Value = serde_json::from_slice(&plain)
                .map_err(|_| invalid("本地网络配置损坏，请在设置中恢复默认。"))?;
            // Preserve old public settings, but never reinterpret a network
            // password as a credential or fall back to password authentication.
            if let Some(fields) = value.as_object_mut() {
                fields.remove("network_secret");
            }
            let mut config: NetworkConfig = serde_json::from_value(value)
                .map_err(|_| invalid("本地网络配置损坏，请在设置中恢复默认。"))?;
            config.normalize_and_validate(false)?;
            Ok(config)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => NetworkConfig::bundled(),
        Err(_) => Err(AppError::new(
            "storage_unavailable",
            "无法读取本地网络配置。",
        )),
    }
}

pub fn save(path: &Path, config: &NetworkConfig) -> Result<(), AppError> {
    let plain = serde_json::to_vec(config).map_err(|_| invalid("无法保存网络配置。"))?;
    let protected = platform::protect(&plain)?;
    platform::atomic_write(path, &protected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> NetworkConfig {
        NetworkConfig {
            network_name: "lab".into(),
            credential_secret: STANDARD.encode([0x42; 32]),
            peers: vec!["tcp://127.0.0.1:11010".into()],
            private_mode: true,
            disable_p2p: true,
            gateway_ip: None,
        }
    }

    #[test]
    fn toml_uses_credential_identity_and_keeps_instance_and_flags() {
        let mut config = fixture();
        config.network_name = "quoted\"\\network".into();
        let text = config.core_toml("测试\"电脑").unwrap();
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(parsed["flags"]["private_mode"].as_bool(), Some(true));
        assert_eq!(parsed["flags"]["disable_p2p"].as_bool(), Some(true));
        assert_eq!(
            parsed["network_identity"]["network_name"].as_str(),
            Some(config.network_name.as_str())
        );
        assert_eq!(parsed["instance_id"].as_str(), Some(INSTANCE_ID));
        assert_eq!(parsed["secure_mode"]["enabled"].as_bool(), Some(true));
        assert_eq!(
            parsed["secure_mode"]["local_private_key"].as_str(),
            Some(config.credential_secret.as_str())
        );
        assert_eq!(
            STANDARD
                .decode(parsed["secure_mode"]["local_public_key"].as_str().unwrap())
                .unwrap()
                .len(),
            32
        );
        assert!(!text.contains("network_secret"));
        assert!(!text.contains("peer_public_key"));
        assert!(!text.contains("credential_file"));
        assert_eq!(
            parsed["peer"][0]["uri"].as_str(),
            Some("tcp://127.0.0.1:11010")
        );
        let view = serde_json::to_value(config.view()).unwrap();
        assert!(view.get("credential_secret").is_none());
        assert!(!view.to_string().contains(&config.credential_secret));
    }

    #[test]
    fn rejects_unsafe_or_invalid_network_configuration() {
        for peer in [
            "",
            "file:///C:/config",
            "tcp://user:password@localhost:11010",
            "tcp://localhost:0",
            "tcp://localhost",
            "tcp://localhost:11010?token=abc",
            "tcp://localhost:11010/arg",
        ] {
            let mut config = fixture();
            config.peers = vec![peer.into()];
            assert!(
                config.normalize_and_validate(true).is_err(),
                "accepted invalid peer"
            );
        }
        let mut config = fixture();
        config.gateway_ip = Some("bad".into());
        assert!(config.normalize_and_validate(true).is_err());
    }

    #[test]
    fn omitted_credential_preserves_same_network_credential() {
        let config = fixture()
            .updated(NetworkConfigUpdate {
                network_name: " lab ".into(),
                credential_secret: None,
                peers: vec!["tcp://localhost:11010".into()],
                private_mode: false,
                disable_p2p: false,
                gateway_ip: Some(" ".into()),
            })
            .unwrap();
        assert_eq!(config.credential_secret, fixture().credential_secret);
        assert_eq!(config.network_name, "lab");
        assert!(!config.private_mode && !config.disable_p2p);
        assert!(config.gateway_ip.is_none());
    }

    fn update(name: &str, credential: Option<&str>) -> NetworkConfigUpdate {
        NetworkConfigUpdate {
            network_name: name.into(),
            credential_secret: credential.map(str::to_owned),
            peers: fixture().peers,
            private_mode: true,
            disable_p2p: true,
            gateway_ip: None,
        }
    }

    #[test]
    fn network_change_requires_explicit_credential_replacement_or_clear() {
        assert!(fixture().updated(update("other", None)).is_err());
        let mut cleared = fixture().updated(update("other", Some(""))).unwrap();
        assert!(!cleared.view().has_credential);
        assert!(cleared.normalize_and_validate(true).is_err());
        assert!(cleared.core_toml("test").is_err());
        assert!(fixture()
            .updated(update("other", Some(&STANDARD.encode([0x55; 32]))))
            .is_ok());
    }

    #[test]
    fn credential_validation_rejects_passwords_and_wrong_lengths_without_echo() {
        for secret in [
            "password",
            "%%%",
            "c2hvcnQ=",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ] {
            let error = fixture()
                .updated(update("lab", Some(secret)))
                .err()
                .unwrap();
            assert!(!error.message.contains(secret));
        }
    }

    #[test]
    fn x25519_public_key_matches_rfc7748_vector() {
        let mut config = fixture();
        config.credential_secret = "dwdtCnMYpX08FsFyUbJmRd9ML4frwJkqsXf7pR25LCo=".into();
        let parsed: toml::Value = toml::from_str(&config.core_toml("test").unwrap()).unwrap();
        assert_eq!(
            parsed["secure_mode"]["local_public_key"].as_str(),
            Some("hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr066SpjqqbTmo=")
        );
    }

    #[test]
    #[cfg(windows)]
    fn dpapi_storage_migrates_old_settings_without_reusing_password() {
        let path =
            std::env::temp_dir().join(format!("rela-credential-test-{}.dat", std::process::id()));
        let mut legacy = serde_json::to_value(fixture()).unwrap();
        legacy.as_object_mut().unwrap().remove("credential_secret");
        legacy["network_secret"] = "legacy-password-must-not-be-used".into();
        platform::atomic_write(
            &path,
            &platform::protect(&serde_json::to_vec(&legacy).unwrap()).unwrap(),
        )
        .unwrap();
        let old = load(&path).unwrap();
        assert!(!old.view().has_credential);
        assert_eq!(old.peers, fixture().peers);
        let next = old
            .updated(update("lab", Some(&fixture().credential_secret)))
            .unwrap();
        save(&path, &next).unwrap();
        let bytes = fs::read(&path).unwrap();
        assert!(!bytes
            .windows(next.credential_secret.len())
            .any(|part| part == next.credential_secret.as_bytes()));
        let plain = platform::unprotect(&bytes).unwrap();
        assert!(!String::from_utf8_lossy(&plain).contains("network_secret"));
        assert!(load(&path).unwrap().view().has_credential);
        fs::remove_file(path).unwrap();
    }
}
