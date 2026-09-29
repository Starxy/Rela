# EasyTier credential 兼容性验证

验证日期：2026-09-29。结论：**Rela 已升级到固定开发版 `2.7.0-0a783c8e`，TOML credential 认证回归和 lab201 现网 no_tun 连接均通过。** Windows 服务和真实 TUN 验收按用户选择暂不执行。

## 当前选定版本与验证

- 官方构建：[Core Actions 35748842247](https://github.com/EasyTier/EasyTier/actions/runs/35748842247)，2026-09-22；提交 `0a783c8e04561d1fee4e3e922e9576402d5bfea3`。
- Core、CLI 均为 `2.7.0-0a783c8e`，属于开发构建。归档 SHA-256：`16c96b7382922940e63fa9ab815d472eea1dd56931d2aaff0ff15d4f4c92dcbb`；单文件校验值见 `config/easytier-version.json`。
- 来源源码包含 [TOML credential 身份识别](https://github.com/EasyTier/EasyTier/blob/0a783c8e04561d1fee4e3e922e9576402d5bfea3/easytier-core/src/config/toml.rs)和[附加 CLI 参数合并](https://github.com/EasyTier/EasyTier/blob/0a783c8e04561d1fee4e3e922e9576402d5bfea3/easytier/src/core.rs)修复。
- 官方 Actions 产物当前保留至 2026-12-21；准备脚本校验原始 ZIP 和所有随包文件，支持通过 `RELA_CORE_ARCHIVE` 指定已保存的原始 ZIP，失效时不会自动追随其他构建。长期归档仍需在正式分发前完成。

| 新版隔离测试                                              | 结果                     |
| --------------------------------------------------------- | ------------------------ |
| 管理节点 TOML 仅 enabled=true，临时 CLI credential 客户端 | 密钥自动初始化，认证成功 |
| 管理节点显式密钥，临时 CLI credential 客户端              | 成功                     |
| 同一凭据写入 TOML                                         | 成功                     |
| 同一 TOML 加 Rela 的启动参数                              | 成功                     |
| TOML 使用未获授权的随机凭据                               | 被拒绝                   |

`npm run test:credential` 对这些结果做断言，失败会非零退出。仅 CLI 对照使用临时生成的隔离测试凭据参数，真实凭据始终通过文件传递。

新版 CLI 同时改用 protobuf JSON：连接成功的默认枚举值 0 会被省略，断开状态为 `DISCONNECTED`。旧测试和 Rela 的整数必填解析曾因此无法正确读取状态，已同步适配；无 URL、未知状态和未就绪 TUN 不会被判定为已连接。

现网验证使用 `src-tauri/examples/credential-tool.rs` 调用 Rela 的实际 `NetworkConfig` 校验和 TOML 序列化代码，由 `scripts/check-live-credential.mjs` 启动新版 Core。结果如下：

- 时间：2026-09-29 03:05 UTC 首次通过，03:17 UTC 保留 Core 默认网卡绑定方式再次通过；客户端 `192.168.200.2/24`，对端 `aliyun_lab201` 为 `192.168.200.1/24`。
- TOML 使用本机 DPAPI 保存的 credential，Secure Mode 开启；固定实例 ID 匹配，无 network_secret、peer_public_key，凭据未进入启动参数。
- 连接持续观察 5 秒，未记录认证拒绝或 ERROR；进程已停止，临时受保护 TOML 已删除，只保留不含凭据的结果 JSON。
- 测试关闭 TUN、STUN、UPnP 和打洞；未修改系统服务、网卡或路由。不能据此认定实际 VPN 流量、服务安装或实验室资源访问通过。

## 历史：2.6.4 的缺陷

### 验证范围和结果

历史验证使用当时随包的 Windows x64 `easytier-core 2.6.4-8428a89d` 和配套 CLI。测试只连接 loopback，开启 `no_tun`，关闭 STUN、UPnP 和打洞；不连接 lab201，不修改系统服务、网卡或路由。管理节点使用独立随机网络与主密码，临时凭据由该管理节点签发。

在管理节点具有有效 Secure Mode 密钥对、开启 `private_mode` 时，对同一份有效临时凭据进行对照：

| 客户端路径                                                              | 结果                                             |
| ----------------------------------------------------------------------- | ------------------------------------------------ |
| `--credential` 参数，Core 自动进入 credential 身份                      | 连接成功                                         |
| TOML 的 `[secure_mode]` 包含凭据私钥及对应公钥，不设置 `network_secret` | 认证失败：`invalid proof and unknown credential` |
| 同一 TOML，加 Rela 使用的 `--no-listener` 等启动参数                    | 同样认证失败                                     |

此处的命令行凭据仅为测试临时生成，不读取或使用用户提供的真实凭据。脚本不输出原始 Core 日志或密钥；结束后停止测试进程并删除临时配置及管理节点凭据文件。`target/credential-compat/` 下只保留不含密钥的结果 JSON。

复现命令：

```powershell
node scripts/check-credential-compat.mjs
```

历史脚本只以 CLI 对照成功作为退出条件。当前脚本已更新为上方新版测试矩阵，对所有预期成功和拒绝结果做断言。

### 根因和 Rela 的影响

2.6.4 的 `TomlConfigLoader::new_from_str` 无条件使用 `NetworkIdentity::new` 重建网络身份，并将未填写的密码补成空字符串。它没有根据 Secure Mode 和无主密码的组合调用 `NetworkIdentity::new_credential`，导致客户端带着错误的身份和密码摘要参与认证。

命令行的 `NetworkOptions::merge_into` 在收到 `--credential` 时会调用 `new_credential`，并将凭据私钥交给 `process_secure_mode_cfg` 生成对应公钥。这说明凭据机制本身可用，两个配置入口的行为不同。

Rela 在 `src-tauri/src/easytier/mod.rs` 中生成受保护的 `core.toml`，随后以 `--config-file` 启动服务；没有使用 `--credential`。因此，仅在当前 `network_config.rs` 的 TOML 输出中增加 Secure Mode 和凭据密钥，仍会失败。调整服务端的 `private_mode` 不能修复客户端的身份加载缺陷。

依据：[上游 Issue #2489](https://github.com/EasyTier/EasyTier/issues/2489)、[v2.6.4 配置加载源码](https://github.com/EasyTier/EasyTier/blob/v2.6.4/easytier/src/common/config.rs#L592)、[v2.6.4 CLI 配置合并源码](https://github.com/EasyTier/EasyTier/blob/v2.6.4/easytier/src/core.rs#L877)。

### 服务端配置的另一个检查点

用户提供的服务端包含主密码、`credential_file`、`secure_mode.enabled = true` 和 `private_mode = true`，角色是管理节点。`credential_file` 用于持久化管理节点签发的凭据，并不代替客户端的 `credential_secret`，也不代替管理节点自己的 Secure Mode 密钥对。

本机另做了服务端对照：2.6.4 仅从 TOML 读取 `[secure_mode] enabled = true`、没有显式密钥对且启动参数不含 `--secure-mode` 时，未生成 Secure Mode 私钥，握手出现 `local private key is not set`。给测试管理节点补齐临时密钥对后，CLI credential 客户端立即能够连接；首次对照中，通过 `--secure-mode` 触发密钥初始化也能连接。

用户后续确认服务端同为 `easytier-core 2.6.4-8428a89d`，systemd 使用 `ExecStart=/usr/local/bin/easytier-core -c /etc/easytier/%i.toml`，不含 `--secure-mode`。版本、配置和启动方式均符合这一独立问题的触发条件，应先处理管理节点的密钥初始化。该判断依据同版本源码和本机复现；尚未登录用户的 Linux 服务器验证实时握手，也未验证已提供凭据的有效期或授权状态。

后续用户补齐服务器密钥配置后，旧版 CLI credential 已成功连接；本轮新版 TOML 也通过现网认证。因此上述服务器初始化问题是历史排查结论，不再列为当前接入阻塞。签发、续期、撤销和重启持久化的完整验收仍待完成。

可在该实例的启动参数中增加 `--secure-mode`，让 2.6.4 的 CLI 配置合并路径初始化管理节点密钥；或者在受保护的服务端 TOML 中配置有效的 Secure Mode 密钥对。不要把客户端 credential 私钥用作管理节点密钥。服务端完成调整后，仍需处理客户端 TOML 身份缺陷，才能让 Rela 使用 credential。实际修改和重启服务器不在本次操作内。

密钥初始化依据：[CLI 的 Secure Mode 处理](https://github.com/EasyTier/EasyTier/blob/v2.6.4/easytier/src/core.rs#L1057)、[SecureModeConfig 密钥读取](https://github.com/EasyTier/EasyTier/blob/v2.6.4/easytier/src/proto/common.rs#L465)。

### 当时的实现决策（已由新版验证结果更新）

- 当前不切换 Rela 的认证实现，因为“现有路径不受影响”的条件不成立。
- 优先选择包含上游修复的 Core/CLI，固定版本和哈希，并用本脚本和 Rela 完整启动参数重新验证。上游 Issue 提及的修复版本信息不能代替对实际分发二进制的验证。
- 不把真实凭据放进进程参数或 Windows 服务启动参数来绕开缺陷。若必须保持 2.6.4，应先单独设计并验证安全的凭据传递方式或维护修复补丁。
- 客户端只使用签发的 credential；服务端主密码不进入源码、客户端配置或安装包。
