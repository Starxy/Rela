//! Real public feed + native store smoke test. No service, TUN, or actual credential.
use rela_lib::distribution::{manager::ResourceManager, DistributionConfig};
use rela_protocol::NetworkConfigUpdate;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/resource-smoke")
        .join(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| "无法读取时钟。")?
                .as_nanos()
                .to_string(),
        );
    tauri::async_runtime::block_on(async {
        let make = || ResourceManager::new(root.clone(), DistributionConfig::bundled()?);
        let manager = make().map_err(|error| error.message)?;
        let initial = manager.status().map_err(|error| error.message)?;
        if initial.configuration_ready {
            return Err("首次状态不应有隐藏默认网络。".into());
        }
        manager
            .refresh(false)
            .await
            .map_err(|error| error.message)?;
        let view = manager
            .store
            .configuration()
            .map_err(|error| error.message)?;
        if view.network_name != "lab201" || view.peers != ["tcp://47.93.55.228:12010"] {
            return Err("线上默认配置不匹配。".into());
        }
        let resources = manager
            .resources(false)
            .await
            .map_err(|error| error.message)?;
        if resources.len() != 1 || resources[0].id != "spark" {
            return Err("Spark 清单缺失。".into());
        }
        manager
            .store
            .save(NetworkConfigUpdate {
                network_name: view.network_name,
                peers: vec!["tcp://127.0.0.1:2222".into()],
                credential_secret: None,
                private_mode: false,
                disable_p2p: false,
                gateway_ip: Some("127.0.0.1".into()),
            })
            .map_err(|error| error.message)?;
        let restarted = make().map_err(|error| error.message)?;
        let local = restarted
            .refresh(false)
            .await
            .map_err(|error| error.message)?;
        if !local.local_override || local.last_checked.is_some() {
            return Err("本地覆盖后发生了自动请求。".into());
        }
        let result = restarted
            .refresh(true)
            .await
            .map_err(|error| error.message)?;
        let restored = restarted
            .store
            .configuration()
            .map_err(|error| error.message)?;
        if result.local_override
            || restored.peers != ["tcp://47.93.55.228:12010"]
            || restored.private_mode
            || restored.disable_p2p
            || restored.gateway_ip.as_deref() != Some("127.0.0.1")
            || restored.has_credential
        {
            return Err("手动更新覆盖或字段保留不符合约定。".into());
        }
        let report = serde_json::json!({ "public_feed_verified": true, "fresh_start_has_no_bundled_network": true,
            "spark_placeholder_loaded": true, "override_survives_restart": true, "automatic_request_skipped": true,
            "manual_refresh_restores_defaults": true, "local_flags_preserved": true, "resource_version": result.resource_version,
            "service_or_tun_modified": false, "time_utc": chrono::Utc::now().to_rfc3339() });
        std::fs::write(
            root.join("result.json"),
            serde_json::to_vec_pretty(&report).map_err(|_| "无法生成报告。")?,
        )
        .map_err(|_| "无法保存报告。")?;
        println!(
            "公开清单与本机覆盖流程验证通过，报告：{}",
            root.join("result.json").display()
        );
        Ok(())
    })
}
