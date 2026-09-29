# Rela

面向实验室成员的 Windows 网络客户端。React + TypeScript 提供 400 × 560 的连接界面，Tauri / Rust 直接管理 EasyTier Core `2.7.0-0a783c8e` 开发版 Windows 服务。

## 当前功能

- 启动时匿名获取并验签公开 GitHub 资源清单，默认网络为 `lab201`、节点为 `tcp://47.93.55.228:12010`。无缓存时等待获取，不内置实验室线路兜底。
- 本地覆盖网络名或节点后暂停清单自动获取，重启后保留；手动“更新线上配置”成功后覆盖线上字段并恢复自动刷新。私有模式、禁用 P2P 和网关仍由本机管理。
- 从有效缓存显示 SSH、Web、NAS 资源；连接后进行 TCP 探测，未连接时显示“未检测”。首份 Spark 仅为占位。
- 独立检查软件稳定/测试渠道，验证签名、架构和版本，保留可信缓存；软件下载安装及恢复流程仍在开发。
- 设置界面可修改网络名称、credential 凭据、连接节点和两个开关，支持更换/清除凭据、恢复默认和保存后重连。
- 使用专用 `RelaEasyTier` 服务连接、断开和重连。GUI 保持普通权限，服务变更由同一程序的一次性提权入口执行。目前每次连接控制都需要 UAC 授权。
- 通过官方 CLI 查询本机 RPC，并核对虚拟网卡地址。只有 Core、节点连接和 TUN 都就绪才显示已连接。
- 可选校园网关 IPv4 检测、实时诊断和不含密钥的摘要导出。
- credential 使用当前 Windows 用户的 DPAPI 加密保存；运行时通过管理员受保护的 TOML 交给 Core，启用 Secure Mode，不传入网络主密码或 `peer_public_key`。已保存凭据不回显至前端，也不进入进程/服务启动参数。
- 最小化或关闭窗口时隐藏到系统托盘；点击托盘图标恢复，右键菜单可打开或退出 Rela。
- 提供安装版和 Portable ZIP 免安装版；便携版设置与界面缓存保存在解压目录。

本项目仅开发客户端，使用现有 EasyTier 网络。自动升级仍在开发；开机启动和自动连接不在本期交付范围。浏览器预览使用演示数据，不代表真实网络验证通过。

## 分发与配置

使用公开仓库 [Starxy/Rela](https://github.com/Starxy/Rela) 分发，客户端无需 GitHub 登录。默认 network 为 `lab201`、peer 为 `tcp://47.93.55.228:12010`，首份资源为 Spark SSH 占位 `192.168.200.10:22`。资源修订 1 已签名、发布并接入客户端；格式与入口见 [清单约定](docs/distribution-format.md)。软件清单入口已确定，测试软件版本尚未发布。

线上清单只管理网络名、节点及资源。私有模式、禁用 P2P、可选网关由本机设置管理。用户覆盖 network 或 peer 后停止清单自动获取，重启后保持；手动点击“更新线上配置”允许覆盖 network/peer 和资源，校验保存成功后恢复自动刷新。软件更新检查独立进行。

凭据由管理员在应用外使用 EasyTier CLI 手动签发，成员在现有设置入口填写。本期不开发凭据生命周期管理、文件导入界面或多网络切换流程。完整约定见 [分发与配置约定](docs/distribution-config-decisions.md)，实现进度见 [todo.md](todo.md)。

## 开发

环境：Windows x64、Node.js 22.12+、Rust 1.88+、Visual Studio C++ Build Tools、Windows SDK、WebView2。参见 [Tauri Windows 前置条件](https://v2.tauri.app/start/prerequisites/#windows)。

```powershell
npm ci
npm run desktop:dev
```

首次运行会从官方 Actions 下载固定开发版 Core，并核对 ZIP 及文件的 SHA-256，需要安装并登录 GitHub CLI。产物保留期限和离线构建方法见 [引擎资产说明](src-tauri/binaries/README.md)。`desktop:dev` 会自行启动前端服务，请先停止占用 1420 端口的其他开发服务。仅看界面可运行 `npm run dev`。

### 网络配置与凭据

`config/network.default.json` 仅保留为公共样本，构建不再读取它。软件只嵌入分发入口、验签公钥和通用参数，不读取 `network.local.json`、`credential.local.dat` 或 `RELA_NETWORK_SECRET`，debug 和 release 均不嵌入真实凭据。

首次使用在设置中粘贴管理员为该网络签发的 credential（32 字节密钥的 Base64 表示）。留空保留同网络已存凭据；更换网络必须同时更换或清除凭据。清除入口需先断开连接；恢复默认会清除用户保存的凭据。未导入有效格式的凭据时，后端拒绝启动连接。

旧版 `network.dat` 自动迁移到新的独立存储：旧主密码被忽略，已有的 Secure Mode credential 可由同一 Windows 用户迁移。原加密文件保留作迁移备份，旧线路保守地视为本地覆盖；手动更新可恢复线上默认。跨用户无法解密时提示重新填写 credential。服务配置在下次连接/重连时更新，服务迁移清理仍待系统验收。客户端不会回退密码认证。

开发者可使用 `cargo build --example credential-tool`，再执行 `target/debug/examples/credential-tool.exe import config/credential.local.dat <用户配置目录>/network.dat` 导入此前保存的 DPAPI 记录。该记录包含 network_name、peers、credential_secret，只能由原 Windows 用户解密；此工具不参与发布包，也不在构建时自动执行。

### 安装版与签名

```powershell
npm run tauri -- build --no-bundle
# RELA_PACKAGE_KEY_FILE 指向仓库外、当前用户 DPAPI 加密的包私钥文件
cargo run -p rela --example package-tool -- bundle "$env:RELA_PACKAGE_KEY_FILE"
```

本地打包器只在固定 Tauri 打包子进程中设置签名密钥环境变量，不写明文密钥或回显打包子进程输出。已配置受保护 Tauri 签名环境的发布端也可直接运行 `npm run desktop:build`。只检查开发构建时使用 `--no-bundle`，无需私钥。

输出位于 `target/release/bundle/nsis/`，包含安装包、Tauri 签名及独立复验元数据。构建/签名通过仍不代表已经完成干净系统上的安装、升级、卸载和真实 VPN 验收。T45 已记录 `Packet.dll` 的内部授权；公开发布包的实际分发范围仍需与授权一致，见 [第三方说明](THIRD-PARTY-NOTICES.md)。

### Portable 免安装版

```powershell
npm run desktop:portable
```

输出为 `target/release/bundle/portable/Rela_0.1.0_x64-portable.zip`，同时生成 ZIP 的 SHA-256 文件。完整解压到有写入权限的本地 NTFS 文件夹，双击 `Rela.exe` 即可。需要已安装 [Microsoft WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。包内含 Core、运行库、许可、使用说明和服务清理工具。

同目录的 `portable.txt` 启用便携模式：设置存入 `data/config`、诊断存入 `data/logs`、WebView2 缓存存入 `data/webview`。移动前退出所有 Rela 界面。已保存凭据绑定当前 Windows 用户，换电脑或用户后须恢复默认并重新导入凭据。

Portable 省去界面安装步骤，首次连接仍需 UAC 授权并创建 `RelaEasyTier` 服务及 `%ProgramData%/Rela`。该服务与安装版共享，退出界面仍保持连接。移除前退出所有 Rela，以管理员身份运行包内 `Remove-Network-Service.cmd`，输入 `REMOVE` 后清理服务及其数据，再删除解压目录。共享网络驱动、系统 WebView2 和安装版用户配置会保留。

仅重新打包已编译程序可运行 `npm run package:portable`；`-- --debug` 用于 CI 的 debug 包。发行前应使用完整 `desktop:portable` 构建，保证程序、前端和引擎资产一致。打包使用全新临时目录和固定文件清单，不包含用户 `data` 或构建配置源文件。

安装版使用更新 ZIP 校验安装器助手和安装后的文件，因此两种包的共用文件必须完全一致。Portable 打包会生成与 Tauri NSIS 相同标记的程序副本，保留原始编译文件；两种包都生成后运行 `npm run check:packages`（调试包加 `-- --debug`）。该检查已加入 Windows CI。构建与验收边界见[产物验证说明](docs/release-validation.md)。

### 检查

```powershell
npm run check          # 前端检查、测试、生产构建
npm run format:check
npm run prepare:core   # cargo 检查前先准备固定版本资产
npm run check:rust     # Rust 格式、Clippy、测试
npm run smoke:core     # 两个仅本机通信的 Core；不创建 TUN、不连接外部网络
npm run test:credential # 隔离 Secure Mode、TOML 认证与错误凭据拒绝
```

CI 使用不带真实密钥的 debug 构建，不连接实验室网络。生产前端默认只接受 Tauri 运行环境；浏览器生产演示须显式设置 `VITE_RELA_PREVIEW=true`。

手动验证现网：先构建 `credential-tool`，再运行 `node scripts/check-live-credential.mjs`。此操作读取本地 DPAPI 凭据并连接记录中的节点，使用 Rela 的 TOML 生成代码，关闭 TUN，验证后清理临时明文文件和进程。测试范围及结果见 [兼容性验证](docs/easytier-credential-compat.md)。

## 运行与数据

首次点击连接会安装 `RelaEasyTier` Windows 服务，并写入经过校验的引擎资产。服务独立于界面。最小化或关闭窗口时隐藏到托盘；左键点击托盘图标或选择“打开 Rela”恢复窗口，“退出 Rela”结束界面程序。上述操作均不停止网络连接，需要断开时使用首页开关。目前服务为手动启动，不会随系统自动连接。

| 数据                | 位置                                                                        |
| ------------------- | --------------------------------------------------------------------------- |
| 网络配置与缓存引用  | Tauri 应用配置目录的 `configuration.json`，原子替换                         |
| 私有 credential     | 同目录 `credentials/*.dat`，当前用户 DPAPI 加密                             |
| 已验签清单          | 同目录 `cache/`，读取时重新核验签名与摘要                                   |
| 软件更新状态        | 同目录 `update-settings.json`、`software-stable.json`、`software-test.json` |
| 设备名称            | 同目录 `preferences.json`                                                   |
| 服务程序、Core 配置 | `%ProgramData%/Rela/`，限制为系统和管理员访问                               |
| 诊断摘要            | Tauri 应用日志目录，导出后显示完整路径                                      |

修改配置后，下次连接或“保存并重连”生效。网关地址可留空；未配置时网关和延迟显示未知。未提供资源清单时桌面端返回空列表。

## 目录

```text
config/                  公共样本、固定引擎版本、分发入口与验签公钥
scripts/                 Core 下载校验、隔离验证、清单签名与发布
packaging/portable/      便携包说明、模式标识和服务清理工具
src/                     React 界面、业务服务与浏览器演示
src-tauri/src/commands/   Tauri 业务命令
src-tauri/src/easytier/   服务生命周期、CLI 与 RPC 状态解析
src-tauri/src/platform/   Windows 权限、DPAPI、SCM、网卡和 ICMP
src-tauri/src/distribution/  签名获取、配置事务、资源探测、软件版本检查
src-tauri/src/network_config.rs  配置校验、旧存储迁移和 Core TOML
src-tauri/installer/      安装器占用检查和共享服务保留钩子
src-tauri/src/updates/    软件更新协调、文件恢复、官方安装器适配
crates/rela-protocol/     共享业务模型
crates/rela-manifests/    严格清单格式、签名与版本校验
docs/                    产品设计、架构、接口、验收清单
```

详见 [架构](docs/architecture.md)、[接口](docs/protocol.md)、[Windows 验收清单](docs/windows-validation.md)。

Core 版本与恢复：设置中区分随附、已部署和正在运行的引擎。服务切换已接入完整文件事务和降级保护，旧密码配置不会自动恢复连接；当前通过文件故障注入及隔离 RPC 测试，真实 Windows 服务/UAC 验收仍待独立机器。设计和恢复边界见 [Core 更新设计](docs/core-update-design.md)。

绿色版和安装版软件更新均已接入确认、下载进度、签名校验、退出交接与启动恢复。安装版通过独立助手运行官方 updater，并协调安装器、用户配置和共享网络服务；两个连续签名版本的实机验收仍待完成，当前包不能视为已验证的自动升级交付。恢复规则与保留的诊断目录见 [本体更新设计](docs/update-recovery-design.md)。

安装器与卸载器检查当前目录是否可替换，并拒绝越过未完成的安装版更新事务，不强制关闭其他 Rela 副本。原用户配置先备份，新版就绪并完成提交后才显示窗口；中断后按持久化记录恢复。安装版首次使用自动更新能力仍需手动安装含新钩子的版本。普通卸载保留各副本共用的网络服务与 ProgramData 配置；需要彻底清理时，先保留安装目录中的 Remove-Network-Service.cmd / .ps1，以管理员身份运行并输入 REMOVE 确认，这会影响本机所有 Rela 副本。升级流程不调用服务删除。
