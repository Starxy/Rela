pub mod manager;
pub mod software;
pub mod store;
pub mod transport;

use rela_manifests::{Channel, PublicKey};
use rela_protocol::AppError;
use serde::Deserialize;
use url::Url;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionConfig {
    pub schema_version: u32,
    pub resources_url: String,
    pub software_stable_url: String,
    pub software_test_url: String,
    pub keys: Vec<PublicKey>,
}

impl DistributionConfig {
    pub fn bundled() -> Result<Self, AppError> {
        let config: Self = serde_json::from_str(include_str!("../../../config/distribution.json"))
            .map_err(|_| manifest_error("分发配置无效。"))?;
        if config.schema_version != 1 {
            return Err(manifest_error("分发配置格式不受支持。"));
        }
        for endpoint in [
            &config.resources_url,
            &config.software_stable_url,
            &config.software_test_url,
        ] {
            let url = Url::parse(endpoint).map_err(|_| manifest_error("清单入口无效。"))?;
            if url.scheme() != "https"
                || url.host_str() != Some("raw.githubusercontent.com")
                || !url.path().starts_with("/Starxy/Rela/distribution/")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.port().is_some()
            {
                return Err(manifest_error("清单入口必须位于公开分发分支。"));
            }
        }
        Ok(config)
    }

    pub fn software_url(&self, channel: Channel) -> &str {
        match channel {
            Channel::Stable => &self.software_stable_url,
            Channel::Test => &self.software_test_url,
        }
    }
}

pub fn manifest_error(message: impl ToString) -> AppError {
    AppError::new("invalid_manifest", &message.to_string())
}
