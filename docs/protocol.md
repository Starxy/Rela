# Rela 本地接口协议

Tauri 业务接口版本：`2`。Rust 模型位于 `crates/rela-protocol/src/lib.rs`，前端镜像位于 `src/types.ts`。JSON 字段使用 `snake_case`；Tauri 调用参数使用命令函数参数名，当前仅有 `id` 和 `preferences`。该版本号不表示 EasyTier RPC 的协议版本。

## Frontend → Native

| Command            | 参数              | 成功返回           | 初始化行为                                |
| ------------------ | ----------------- | ------------------ | ----------------------------------------- |
| `get_status`       | 无                | `ConnectionStatus` | 断开、Core 不可用，无虚拟 IP              |
| `connect`          | 无                | `ConnectionStatus` | `core_unavailable` 错误                   |
| `disconnect`       | 无                | `ConnectionStatus` | `core_unavailable` 错误                   |
| `reconnect`        | 无                | `ConnectionStatus` | `core_unavailable` 错误                   |
| `get_resources`    | 无                | `LabResource[]`    | 空列表                                    |
| `open_resource`    | `{ id }`          | `null`             | `resource_unavailable` 或参数错误         |
| `run_diagnostics`  | 无                | `DiagnosticReport` | Core 未接入，其余检查跳过                 |
| `export_logs`      | 无                | `string`           | 写入诊断摘要 JSON，返回保存路径提示       |
| `get_version`      | 无                | `VersionInfo`      | 应用版本、目标 Core 版本、协议版本        |
| `get_preferences`  | 无                | `Preferences`      | 读取非敏感设置，不存在时返回默认值        |
| `save_preferences` | `{ preferences }` | `Preferences`      | 校验并保存设备名称；自动启动/连接暂不支持 |

`export_logs` 保留产品设计中的命令名，但当前只生成诊断摘要，不是完整日志包。原始 Core 日志尚未收集。示例资源与正常网关数据仅存在于浏览器演示服务。

### 连接状态

```json
{
  "connected": false,
  "core": "unavailable",
  "virtual_ip": null,
  "gateway": "unknown",
  "latency_ms": null,
  "connection_type": null,
  "resources_available": 0,
  "resources_total": 0,
  "last_error": "网络引擎尚未接入。"
}
```

- `core`：`unavailable | stopped | running`。分别表示不可用、已停止、运行中；运行中不等于已经连接实验室网络。
- `gateway`：`unknown | online | offline`。
- `connection_type`：`direct | relay | null`。
- 无数据用 `null` 或 `unknown`，不能用虚假的 IP、零延迟或“正常”代替。
- 资源数由实际业务探测统计，不从 EasyTier 节点数量推断。

### 错误

Tauri `Result` 的错误分支通过 `invoke` rejection 返回：

```json
{
  "code": "core_unavailable",
  "message": "网络引擎尚未接入，暂时无法控制连接。"
}
```

错误信息是面向用户的说明。底层命令、凭据、未脱敏日志不得作为 `message` 原样返回。

### 其他模型

- `LabResource`：`id`、`name`、`kind`（`ssh | web | nas`）、`description`、`address`、`availability`（`unknown | reachable | unreachable`）。
- `DiagnosticReport`：`generated_at`（RFC 3339）、`summary`、`checks`。检查含 `id`、`label`、`level`（`pass | warning | error | skipped`）、`message`。
- `Preferences`：`device_name`、`launch_at_login`、`auto_connect`。不包含 Machine ID、Config Server token 或 Device Credential。
- `VersionInfo`：`app`、`easytier_target`、`easytier_installed`、`protocol`。未安装时 `easytier_installed = null`。

## Native → EasyTier Core

Tauri 的连接命令直接调用同一进程内的 `EasyTierCore` 模块，不经过自定义后台进程。Core 生命周期和本机 RPC 由该模块接入；Windows 服务控制由 `platform` 模块提供。实验室资源由 Rela 管理，不从 Core 节点列表推断。

当前 `get_status` 返回不可用状态，`connect`、`disconnect`、`reconnect` 返回 `core_unavailable`。实际服务控制和 RPC 尚未实现。

后续接入需验证：

1. 固定版本 Core 的 RPC 方法、响应模型及本机调用授权；只监听 `127.0.0.1`。
2. 安装时提权、普通用户服务控制权限、配置和二进制文件访问权限。
3. 调用超时、取消、幂等连接 / 断开，以及并发控制和状态事件。
4. Core 服务的失败恢复、系统唤醒处理，以及界面退出后连接继续运行。
5. 业务状态与实际网关和资源探测的一致性、日志脱敏及权限测试。

状态、错误和诊断由 Rust 后端转成上面的业务模型，前端不接收 Core 的原始控制参数或凭据。
