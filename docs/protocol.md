# Rela 本地接口协议

Tauri 业务接口版本 `4`；JSON 字段使用 `snake_case`。Rust 定义位于 `crates/rela-protocol/src/lib.rs`，前端镜像位于 `src/types.ts`。此版本独立于 EasyTier RPC 协议。

## Frontend → Native

| Command                | 参数              | 返回与行为                                  |
| ---------------------- | ----------------- | ------------------------------------------- |
| `get_status`           | 无                | `ConnectionStatus`，读取实际服务和 RPC      |
| `connect`              | 无                | 安装或启动专用服务，返回最新状态            |
| `disconnect`           | 无                | 停止专用服务，返回最新状态                  |
| `reconnect`            | 无                | 使用已保存配置重新启动，返回最新状态        |
| `get_network_config`   | 无                | `NetworkConfigView`，隐藏已保存密钥         |
| `save_network_config`  | `{ config }`      | 校验并保存 Update，返回 View；下次连接生效  |
| `reset_network_config` | 无                | 恢复构建默认值，返回 View；下次连接生效     |
| `get_resources`        | 无                | `LabResource[]`，当前桌面端为空             |
| `open_resource`        | `{ id }`          | 当前返回 `resource_unavailable`             |
| `run_diagnostics`      | 无                | `DiagnosticReport`，从实际状态构建          |
| `export_logs`          | 无                | 保存诊断摘要 JSON，返回路径提示             |
| `get_version`          | 无                | `VersionInfo`，含应用、目标及随附 Core 版本 |
| `get_preferences`      | 无                | `Preferences`，读取设备名称和偏好           |
| `save_preferences`     | `{ preferences }` | 保存设备名称；自动连接和开机启动暂不支持    |

### 网络配置

View 包含 `network_name`、`has_credential`、`peers: string[]`、`private_mode`、`disable_p2p`、`gateway_ip: string | null`。

Update 使用相同配置字段，但把 `has_credential` 换成可选 `credential_secret`。省略或 null 保留同网络的原凭据；空字符串清除；非空值必须为 Base64 编码的 32 字节 credential，首尾空白会被去除。网络名改变时必须显式更换或清除凭据，不自动跨网络复用。前端仅在用户输入时短暂持有新凭据，保存后清空。

允许保存尚未导入凭据的公共配置；连接、重连和 helper 都要求存在有效格式的凭据。恢复默认会清除凭据。旧版 network_secret 不再接受为 Update 字段；旧存储只读取公共字段，重新导入并保存后覆盖。服务器授权状态仍由握手决定，不能仅凭格式校验判定有效期和撤销状态。

节点必须是合法的 tcp / udp / quic / ws / wss 地址，不能含用户信息、查询参数或片段。网关可留空，否则须为 IPv4。

### 连接状态

```json
{
  "connected": false,
  "core": "stopped",
  "virtual_ip": null,
  "gateway": "unknown",
  "latency_ms": null,
  "connection_type": null,
  "resources_available": 0,
  "resources_total": 0,
  "last_error": null
}
```

- `core`: `unavailable | stopped | starting | stopping | running`。
- `gateway`: `unknown | online | offline`。
- `connection_type`: `direct | relay | null`，根据目标网关路由判断。
- 服务运行不等于已连接；已连接不等于实验室网关或资源可达。
- 无数据使用 null / unknown，不用示例数据代替。状态读取错误会清除界面上的旧连接状态。

### 错误

Tauri Result 的错误分支以 `{ code, message }` rejection 返回。常见错误包括 `invalid_network_config`、`credential_storage_failed`、`core_integrity_failed`、`permission_cancelled`、`service_conflict`、`service_busy`、`service_action_failed`、`service_timeout`。提权子进程只返回有限退出码，无法识别的底层错误统一为面向用户的操作失败信息；不返回原始输出或密钥。

### 其他模型

- `LabResource`: id、name、kind（ssh / web / nas）、description、address、availability。
- `DiagnosticReport`: generated_at（RFC 3339）、summary、checks。每项含 id、label、level（pass / warning / error / skipped）、message。
- `Preferences`: device_name、launch_at_login、auto_connect。
- `VersionInfo`: app、easytier_target、easytier_installed、protocol。installed 表示随附资产经校验可执行，并非服务已经安装。

## Native → Core

SCM 管理固定服务 `RelaEasyTier`；官方 CLI 用 `--instance-name rela --output json` 查询 `127.0.0.1:35888` 的 node / connector / route。CLI 返回值转换为上述业务模型。命令执行有输出上限和超时，不将 Core 的完整配置返回前端。

Core 固定为 `2.7.0-0a783c8e`。TOML 使用 Secure Mode 的 credential 私钥与推导公钥，无 `network_secret` 或 `peer_public_key`；凭据不进入服务启动参数。实例 ID 保持 `5f9e7c9b-747a-47e5-b62b-3c2b607c312e`。

2.7 CLI 的 connector JSON 省略默认的 `status=0`，断开/连接中状态为 `DISCONNECTED`/`CONNECTING`。Native 仅在存在非空 URL、状态为省略/0/CONNECTED 时认定节点已连接；未知枚举不算连接，缺少 URL 的对象视为无效响应。无 IPv4 的 node 可省略 ipv4_addr，按等待分配处理。

服务操作串行执行。连接已运行的服务为幂等操作，不主动覆盖运行配置；应用新配置应使用重连。首次连接为手动启动服务，关闭 GUI 不会停止服务。
