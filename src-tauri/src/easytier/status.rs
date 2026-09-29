use rela_protocol::{ConnectionStatus, ConnectionType, CoreState};
use serde::Deserialize;
use std::net::Ipv4Addr;

/// Deliberately ignore NodeInfo.config, which contains credentials.
#[derive(Deserialize)]
pub struct NodeInfo {
    #[serde(default)]
    pub ipv4_addr: String,
    pub inst_id: String,
}

#[derive(Deserialize)]
#[serde(try_from = "ConnectorPayload")]
pub struct Connector {
    pub status: i32,
}

#[derive(Deserialize)]
struct ConnectorPayload {
    url: ConnectorUrl,
    // 2.7.0 CLI uses protobuf JSON: Connected = 0 is omitted.
    #[serde(default)]
    status: ConnectorStatus,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ConnectorStatus {
    Number(i32),
    Name(String),
}

impl Default for ConnectorStatus {
    fn default() -> Self {
        Self::Number(0)
    }
}

#[derive(Deserialize)]
struct ConnectorUrl {
    url: String,
}

impl TryFrom<ConnectorPayload> for Connector {
    type Error = &'static str;

    fn try_from(value: ConnectorPayload) -> Result<Self, Self::Error> {
        if value.url.url.is_empty() {
            return Err("connector URL is missing");
        }
        let connected = match value.status {
            ConnectorStatus::Number(number) => number == 0,
            ConnectorStatus::Name(name) => name == "CONNECTED",
        };
        Ok(Self {
            status: if connected { 0 } else { 1 },
        })
    }
}

#[derive(Deserialize)]
pub struct RouteRow {
    pub ipv4: String,
    pub proxy_cidrs: String,
    pub path_len: i32,
}

pub fn parse_ipv4(value: &str) -> Option<Ipv4Addr> {
    value
        .split('/')
        .next()?
        .parse::<Ipv4Addr>()
        .ok()
        .filter(|ip| !ip.is_unspecified())
}

pub fn connection_snapshot(
    node: &NodeInfo,
    connectors: &[Connector],
    tun_ready: bool,
) -> ConnectionStatus {
    let ip = parse_ipv4(&node.ipv4_addr);
    let peer_connected = connectors.iter().any(|connector| connector.status == 0);
    let connected = ip.is_some() && tun_ready && peer_connected;
    ConnectionStatus {
        connected,
        core: CoreState::Running,
        virtual_ip: ip.filter(|_| tun_ready).map(|ip| ip.to_string()),
        last_error: if ip.is_none() {
            Some("正在获取虚拟 IP。".into())
        } else if !tun_ready {
            Some("虚拟网卡尚未就绪，请检查系统权限或重新连接。".into())
        } else if !peer_connected {
            Some("正在连接配置的网络节点。".into())
        } else {
            None
        },
        ..Default::default()
    }
}

pub fn gateway_connection_type(routes: &[RouteRow], gateway: Ipv4Addr) -> Option<ConnectionType> {
    let route = routes
        .iter()
        .filter(|route| route.path_len > 0)
        .find(|route| parse_ipv4(&route.ipv4) == Some(gateway))
        .or_else(|| {
            routes
                .iter()
                .filter(|route| route.path_len > 0)
                .find(|route| {
                    route
                        .proxy_cidrs
                        .split(',')
                        .any(|cidr| contains_ip(cidr.trim(), gateway))
                })
        })?;
    Some(if route.path_len == 1 {
        ConnectionType::Direct
    } else {
        ConnectionType::Relay
    })
}

fn contains_ip(cidr: &str, target: Ipv4Addr) -> bool {
    let Some((address, prefix)) = cidr.split_once('/') else {
        return false;
    };
    let (Ok(address), Ok(prefix)) = (address.parse::<Ipv4Addr>(), prefix.parse::<u32>()) else {
        return false;
    };
    if prefix > 32 {
        return false;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    u32::from(address) & mask == u32::from(target) & mask
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protobuf_json_omits_connected_status_but_invalid_rows_are_rejected() {
        let connected: Connector =
            serde_json::from_str(r#"{"url":{"url":"tcp://127.0.0.1:11010"}}"#).unwrap();
        assert_eq!(connected.status, 0);
        let pending: Connector =
            serde_json::from_str(r#"{"url":{"url":"tcp://127.0.0.1:11010"},"status":1}"#).unwrap();
        assert_eq!(pending.status, 1);
        let disconnected: Connector = serde_json::from_str(
            r#"{"url":{"url":"tcp://127.0.0.1:11010"},"status":"DISCONNECTED"}"#,
        )
        .unwrap();
        assert_eq!(disconnected.status, 1);
        let connected_named: Connector =
            serde_json::from_str(r#"{"url":{"url":"tcp://127.0.0.1:11010"},"status":"CONNECTED"}"#)
                .unwrap();
        assert_eq!(connected_named.status, 0);
        assert!(serde_json::from_str::<Connector>("{}").is_err());
        assert!(serde_json::from_str::<Connector>(r#"{"url":{"url":""}}"#).is_err());
        let starting: NodeInfo = serde_json::from_str(r#"{"inst_id":"test"}"#).unwrap();
        assert!(!connection_snapshot(&starting, &[connected], true).connected);
    }
    #[test]
    fn core_and_ip_alone_do_not_prove_connectivity() {
        let node = NodeInfo {
            ipv4_addr: "10.1.2.3/24".into(),
            inst_id: "test".into(),
        };
        assert!(!connection_snapshot(&node, &[], true).connected);
        assert!(!connection_snapshot(&node, &[Connector { status: 0 }], false).connected);
        assert!(!connection_snapshot(&node, &[Connector { status: 1 }], true).connected);
        assert!(connection_snapshot(&node, &[Connector { status: 0 }], true).connected);
    }
    #[test]
    fn node_payload_credentials_do_not_enter_business_status() {
        let node: NodeInfo = serde_json::from_str(
            r#"{"ipv4_addr":"10.1.2.3","inst_id":"test","config":"network_secret = confidential"}"#,
        )
        .unwrap();
        let json = serde_json::to_string(&connection_snapshot(
            &node,
            &[Connector { status: 0 }],
            true,
        ))
        .unwrap();
        assert!(!json.contains("confidential"));
        assert!(!json.contains("network_secret"));
    }
    #[test]
    fn gateway_path_uses_real_route_and_not_the_p2p_setting() {
        let rows = vec![RouteRow {
            ipv4: "10.1.2.1".into(),
            proxy_cidrs: "10.20.0.0/16".into(),
            path_len: 2,
        }];
        assert!(matches!(
            gateway_connection_type(&rows, "10.20.1.1".parse().unwrap()),
            Some(ConnectionType::Relay)
        ));
        assert!(gateway_connection_type(&rows, "192.168.1.1".parse().unwrap()).is_none());
    }
}
