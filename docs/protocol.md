# Rela 本地接口协议

Tauri 业务接口版本 `3`；JSON 字段使用 `snake_case`。Rust 定义位于 `crates/rela-protocol/src/lib.rs`，前端镜像位于 `src/types.ts`。此版本独立于 EasyTier RPC 协议。

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

View 包含 `network_name`、`has_network_secret`、`peers: string[]`、`private_mode`、`disable_p2p`、`gateway_ip: string | null`。

Update 使用相同配置字段，但把 `has_network_secret` 换成可选 `network_secret`。省略或 null 保留原密钥；传入空字符串会被拒绝。前端仅在用户输入时短暂持有新密钥，保存后清空。节点必须是合法的 tcp / udp / quic / ws / wss 地址，不能含用户信息、查询参数或片段。网关可留空，否则须为 IPv4。

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

服务操作串行执行。连接已运行的服务为幂等操作，不主动覆盖运行配置；应用新配置应使用重连。首次连接为手动启动服务，关闭 GUI 不会停止服务。
