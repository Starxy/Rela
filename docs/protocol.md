# Rela 本地接口协议

Tauri 业务接口版本 `8`；JSON 字段使用 `snake_case`。Rust 定义位于 `crates/rela-protocol/src/lib.rs`，前端镜像位于 `src/types.ts`。此版本独立于 EasyTier RPC 协议。

## Frontend → Native

| Command                   | 参数                              | 返回与行为                                                   |
| ------------------------- | --------------------------------- | ------------------------------------------------------------ |
| `get_status`              | 无                                | `ConnectionStatus`，读取实际服务和 RPC                       |
| `connect`                 | 无                                | 安装或启动专用服务，返回最新状态                             |
| `disconnect`              | 无                                | 停止专用服务，返回最新状态                                   |
| `reconnect`               | 无                                | 使用已保存配置重新启动，返回最新状态                         |
| `get_network_config`      | 无                                | `NetworkConfigView`，隐藏已保存密钥                          |
| `save_network_config`     | `{ config }`                      | 校验并保存 Update，返回 View；下次连接生效                   |
| `reset_network_config`    | 无                                | 清除凭据与本机覆盖，恢复可信缓存默认值并后台刷新             |
| `get_resources`           | 无                                | `LabResource[]`，连接后执行有界 TCP 探测                     |
| `open_resource`           | `{ id }`                          | 要求真实连接，按有效清单打开 SSH、Web 或 NAS 文件夹          |
| `get_resource_sync`       | 无                                | `ResourceSyncStatus`，读取配置来源与生效状态                 |
| `refresh_resources`       | 无                                | 手动更新线上 network/peer/resources，成功后恢复自动刷新      |
| `get_software_update`     | 无                                | `SoftwareUpdateStatus`，独立软件检查状态与候选版本           |
| `check_software_update`   | 无                                | 匿名获取并验签所选渠道的软件清单，不下载或安装包             |
| `set_update_channel`      | `{ channel: "stable" 或 "test" }` | 持久化渠道并返回对应缓存状态，检查期间拒绝切换               |
| `get_update_progress`     | 无                                | `UpdateProgress`，绿色版下载、准备、重启或失败状态           |
| `install_software_update` | `{ version }`                     | 重新验证用户确认的版本并启动绿色版更新，成功交接后退出原 GUI |
| `complete_update_startup` | 无                                | 前端挂载后发送候选启动确认，返回更新/回退提示或 null         |
| `run_diagnostics`         | 无                                | `DiagnosticReport`，从实际状态构建                           |
| `export_logs`             | 无                                | 保存诊断摘要 JSON，返回路径提示                              |
| `get_version`             | 无                                | `VersionInfo`，含应用、随附、已部署及运行 Core 版本          |
| `get_preferences`         | 无                                | `Preferences`，读取设备名称和偏好                            |
| `save_preferences`        | `{ preferences }`                 | 保存设备名称；自动连接和开机启动暂不支持                     |

### 网络配置

View 包含 `network_name`、`has_credential`、`peers: string[]`、`private_mode`、`disable_p2p`、`gateway_ip: string | null`。

Update 使用相同配置字段，但把 `has_credential` 换成可选 `credential_secret`。省略或 null 保留同网络的原凭据；空字符串清除；非空值必须为 Base64 编码的 32 字节 credential，首尾空白会被去除。网络名改变时必须显式更换或清除凭据，不自动跨网络复用。前端仅在用户输入时短暂持有新凭据，保存后清空。

允许保存尚未导入凭据的公共配置；连接、重连和 helper 都要求存在有效格式的凭据。恢复默认会清除凭据。旧版 network_secret 不再接受为 Update 字段；旧密码不复用，旧加密文件保留作迁移备份。服务器授权状态仍由握手决定，不能仅凭格式校验判定有效期和撤销状态。

节点必须是合法的 tcp / udp / quic / ws / wss 地址，不能含用户信息、查询参数或片段。网关可留空，否则须为 IPv4。

### 线上配置与软件检查

- 远程清单使用 `schema_version`、`version`、`network`、`peer`、`resources`，`network`/`peer` 映射到本地 `network_name`/`peers`。样本见 [resources.example.json](../config/resources.example.json)。
- 远程不管理 `private_mode`、`disable_p2p`、`gateway_ip`、设备偏好或 credential。
- 本地 network/peer 覆盖状态由 Native 持久化。存在覆盖时，自动检查不得发起线上清单请求；保存本地配置后，已经在途的自动请求不得覆盖较新的修改。
- 手动更新允许覆盖清单管理的 network/peer 和资源，成功保存后清除覆盖状态并恢复自动刷新；失败保留本地修改。调用方不能借此设置本机开关、网关或绕过凭据绑定校验。
- 本机覆盖不影响软件版本检查。资源可用性由真实探测生成，Spark 占位不等于可达。
- 本期保留 credential 填写即可，不要求文件导入接口、凭据生命周期管理或多网络管理。

`ResourceSyncStatus` 包含 local_override、resource_version、applied_resource_version、pending_reconnect、configuration_ready、credential_unavailable、has_credential、refreshing、last_checked、last_error。服务实际应用后才记录 applied；幂等连接已有服务不把待应用配置标成生效。

`SoftwareUpdateStatus` 包含 channel、install_kind（installer/portable）、current_version、checking、last_checked、last_error、candidate、cached。candidate 包含 version、notes、published_at、size、core_version、requires_manual_upgrade；无候选不等于检查成功，需结合 last_checked/last_error。失败保留可信缓存并标记 cached。客户端不接受前端传入包 URL 或签名，绿色版通过用户确认的 version 选择可信缓存；版本改变时拒绝旧确认。同版本的包、Core 和最低升级要求不可随清单新修订改变。安装版已具备官方 updater 的验证适配层，但安装事务和业务命令接入仍待完成。

`UpdateProgress` 包含 phase（idle/downloading/preparing/restarting/failed）、version、downloaded、total、error。下载完成不代表安装完成。候选启动确认前隐藏窗口并禁止配置、清单和网络操作，确认提交后再启动后台刷新；失败退出，由助手恢复。

完整规则见[分发与配置约定](distribution-config-decisions.md)和[签名清单格式](distribution-format.md)。

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

Tauri Result 的错误分支以 `{ code, message }` rejection 返回。常见错误包括 `invalid_network_config`、`credential_storage_failed`、`core_integrity_failed`、`permission_cancelled`、`service_conflict`、`service_busy`、`service_action_failed`、`service_timeout`、`core_downgrade_blocked`、`core_update_rolled_back`、`core_recovery_required`、`credential_migration_required`、`core_version_unknown`。提权子进程只返回有限退出码，无法识别的底层错误统一为面向用户的操作失败信息；不返回原始输出或密钥。

### 其他模型

- `LabResource`: id、name、kind（ssh / web / nas）、description、address、availability。
- `DiagnosticReport`: generated_at（RFC 3339）、summary、checks。每项含 id、label、level（pass / warning / error / skipped）、message。
- `Preferences`: device_name、launch_at_login、auto_connect。
- `VersionInfo`: app、easytier_target、easytier_bundled、easytier_deployed、easytier_running、engine_owner_app、engine_revision、protocol。bundled 通过随包摘要校验；deployed 从专用服务的部署元数据读取；running 来自运行服务对应实例的实际 RPC。未知值为 null，三种版本不可互相代替。

## Native → Core

SCM 管理固定服务 `RelaEasyTier`；官方 CLI 用 `--instance-name rela --output json` 查询 `127.0.0.1:35888` 的 node / connector / route。CLI 返回值转换为上述业务模型。命令执行有输出上限和超时，不将 Core 的完整配置返回前端。

Core 固定为 `2.7.0-0a783c8e`。TOML 使用 Secure Mode 的 credential 私钥与推导公钥，无 `network_secret` 或 `peer_public_key`；凭据不进入服务启动参数。实例 ID 保持 `5f9e7c9b-747a-47e5-b62b-3c2b607c312e`。

2.7 CLI 的 connector JSON 省略默认的 `status=0`，断开/连接中状态为 `DISCONNECTED`/`CONNECTING`。Native 仅在存在非空 URL、状态为省略/0/CONNECTED 时认定节点已连接；未知枚举不算连接，缺少 URL 的对象视为无效响应。无 IPv4 的 node 可省略 ipv4_addr，按等待分配处理。

服务操作串行执行。连接已运行的服务为幂等操作，不主动覆盖运行配置；应用新配置应使用重连。首次连接为手动启动服务，关闭 GUI 不会停止服务。

引擎切换使用受保护的 `engine-v2` 目录与事务日志。每次先恢复中断事务，再做版本守卫、完整暂存、备份、注册与启动校验；旧副本不得降低部署所属应用版本或引擎修订。恢复失败保留备份，断开操作的恢复阶段不重启连接；旧密码配置永不自动重启。详见 [Core 更新设计](core-update-design.md)。
