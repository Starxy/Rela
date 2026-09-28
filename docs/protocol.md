# Rela 本地接口协议

协议版本：`1`。Rust 模型位于 `crates/rela-protocol/src/lib.rs`，前端镜像位于 `src/types.ts`。JSON 字段使用 `snake_case`；Tauri 调用参数使用命令函数参数名，当前仅有 `id` 和 `preferences`。

## Frontend → Native

| Command            | 参数              | 成功返回           | 初始化行为                                |
| ------------------ | ----------------- | ------------------ | ----------------------------------------- |
| `get_status`       | 无                | `ConnectionStatus` | 断开、Agent 不可用，无虚拟 IP             |
| `connect`          | 无                | `ConnectionStatus` | `agent_unavailable` 错误                  |
| `disconnect`       | 无                | `ConnectionStatus` | `agent_unavailable` 错误                  |
| `reconnect`        | 无                | `ConnectionStatus` | `agent_unavailable` 错误                  |
| `get_resources`    | 无                | `LabResource[]`    | 空列表                                    |
| `open_resource`    | `{ id }`          | `null`             | `resource_unavailable` 或参数错误         |
| `run_diagnostics`  | 无                | `DiagnosticReport` | 后台未接入，其余检查跳过                  |
| `export_logs`      | 无                | `string`           | 写入诊断摘要 JSON，返回保存路径提示       |
| `get_version`      | 无                | `VersionInfo`      | 应用版本、目标 Core 版本、协议版本        |
| `get_preferences`  | 无                | `Preferences`      | 读取非敏感设置，不存在时返回默认值        |
| `save_preferences` | `{ preferences }` | `Preferences`      | 校验并保存设备名称；自动启动/连接暂不支持 |

`export_logs` 保留产品设计中的命令名，但当前只生成诊断摘要，不是完整日志包。原始 Core 日志尚未收集。示例资源与正常网关数据仅存在于浏览器演示服务。

### 连接状态

```json
{
  "connected": false,
  "agent": "unavailable",
  "virtual_ip": null,
  "gateway": "unknown",
  "latency_ms": null,
  "connection_type": null,
  "resources_available": 0,
  "resources_total": 0,
  "last_error": "后台服务尚未接入。"
}
```

- `agent`：`unavailable | ready`。
- `gateway`：`unknown | online | offline`。
- `connection_type`：`direct | relay | null`。
- 无数据用 `null` 或 `unknown`，不能用虚假的 IP、零延迟或“正常”代替。
- 资源数由实际业务探测统计，不从 EasyTier 节点数量推断。

### 错误

Tauri `Result` 的错误分支通过 `invoke` rejection 返回：

```json
{
  "code": "agent_unavailable",
  "message": "后台服务尚未接入。请等待服务安装功能完成后连接。"
}
```

错误信息是面向用户的说明。底层命令、凭据、未脱敏日志不得作为 `message` 原样返回。

### 其他模型

- `LabResource`：`id`、`name`、`kind`（`ssh | web | nas`）、`description`、`address`、`availability`（`unknown | reachable | unreachable`）。
- `DiagnosticReport`：`generated_at`（RFC 3339）、`summary`、`checks`。检查含 `id`、`label`、`level`（`pass | warning | error | skipped`）、`message`。
- `Preferences`：`device_name`、`launch_at_login`、`auto_connect`。不包含 Machine ID、Config Server token 或 Device Credential。
- `VersionInfo`：`app`、`easytier_target`、`easytier_installed`、`protocol`。未安装时 `easytier_installed = null`。

## Native → Agent（待实现）

预留 Windows 管道名：`\\.\pipe\rela-agent-v1`。目前不存在可连接的服务端，也未实现 IPC 客户端；`AgentClient` 返回不可用状态。

共享 crate 目前定义以下请求形状，用于后续传输层开发：

```json
{
  "protocol_version": 1,
  "request_id": "request-identifier",
  "method": "get_status"
}
```

`method` 限定为 `get_status | connect | disconnect | reconnect | get_resources | run_diagnostics`。模型拒绝未知方法和额外字段。当前序列化模型不承担认证或协议版本验证。

实现传输层之前，必须确定：

1. Named Pipe ACL、本地调用者身份和每用户设备授权。
2. 消息分帧、大小上限、请求超时和断开取消。
3. 协议版本检查、请求 ID 回传、类型化响应和错误封装。
4. 幂等连接 / 断开、并发请求排队和事件订阅。
5. 服务崩溃恢复、日志脱敏及权限越界测试。

不能将预留的管道名称或 JSON 类型视为已完成的安全通信协议。
