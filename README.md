# Rela

面向实验室成员的 Windows 网络客户端。React + TypeScript 提供 400 × 560 的连接界面，Tauri / Rust 直接管理 EasyTier Core 2.6.4 Windows 服务。

## 当前功能

- 内置构建时网络配置，默认网络为 `lab201`、节点为 `tcp://47.93.55.228:12010`，默认启用 `private_mode` 和 `disable_p2p`。
- 设置界面可修改网络名称、密钥、连接节点和两个开关，支持恢复默认和保存后重连。
- 使用专用 `RelaEasyTier` 服务连接、断开和重连。GUI 保持普通权限，服务变更由同一程序的一次性提权入口执行。目前每次连接控制都需要 UAC 授权。
- 通过官方 CLI 查询本机 RPC，并核对虚拟网卡地址。只有 Core、节点连接和 TUN 都就绪才显示已连接。
- 可选校园网关 IPv4 检测、实时诊断和不含密钥的摘要导出。
- 修改后的配置使用当前 Windows 用户的 DPAPI 加密保存；已保存密钥不回显至前端。
- 最小化或关闭窗口时隐藏到系统托盘；点击托盘图标恢复，右键菜单可打开或退出 Rela。
- 提供安装版和 Portable ZIP 免安装版；便携版设置与界面缓存保存在解压目录。

本项目仅开发客户端，使用现有 EasyTier 网络。资源列表、开机启动、自动连接和自动升级仍待开发。浏览器预览使用演示数据，不代表真实网络验证通过。

## 开发

环境：Windows x64、Node.js 22.12+、Rust 1.88+、Visual Studio C++ Build Tools、Windows SDK、WebView2。参见 [Tauri Windows 前置条件](https://v2.tauri.app/start/prerequisites/#windows)。

```powershell
npm ci
npm run desktop:dev
```

首次运行会从官方发布页下载固定版本 Core，并核对 SHA-256。`desktop:dev` 会自行启动前端服务，请先停止占用 1420 端口的其他开发服务。仅看界面可运行 `npm run dev`。

### 构建默认网络

`config/network.default.json` 提交公共默认值，不包含真实密钥。开发者可以将它复制为被 Git 忽略的 `config/network.local.json`，填写完整配置。构建时优先使用本地文件；环境变量 `RELA_NETWORK_SECRET` 可覆盖其中的密钥。

`lab201` 使用 credential 接入，当前客户端的认证实现仍只支持 `network_secret`，需按 [待办](todo.md) 完成 Secure Mode 改造后才能连接。准备中的 credential 可保存在 Git 忽略的 `config/credential.local.dat`，由当前 Windows 用户的 DPAPI 加密；当前程序和构建脚本尚不读取此文件，不要将 credential 填入 `network_secret`。

发布构建要求提供非空密钥。不要把真实密钥填回默认文件或命令示例。构建默认值只嵌入 Native 程序，不写入前端静态文件。任何拿到安装包的人仍可提取共享密钥，这符合当前共享网络方案，不能当作设备独立凭据。

```powershell
npm run desktop:build
```

输出位于 `target/release/bundle/nsis/`。该包当前用于内部测试；尚未签名，也未完成干净系统上的安装、升级、卸载和真实 VPN 验收。上游随附 `Packet.dll` 的再分发许可也是正式发布前的待解决项，见 [第三方说明](THIRD-PARTY-NOTICES.md)。

### Portable 免安装版

```powershell
npm run desktop:portable
```

输出为 `target/release/bundle/portable/Rela_0.1.0_x64-portable.zip`，同时生成 ZIP 的 SHA-256 文件。完整解压到有写入权限的本地 NTFS 文件夹，双击 `Rela.exe` 即可。需要已安装 [Microsoft WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。包内含 Core、运行库、许可、使用说明和服务清理工具。

同目录的 `portable.txt` 启用便携模式：设置存入 `data/config`、诊断存入 `data/logs`、WebView2 缓存存入 `data/webview`。移动前退出所有 Rela 界面。已保存网络密钥绑定当前 Windows 用户，换电脑或用户后须恢复默认或重新填写配置。

Portable 省去界面安装步骤，首次连接仍需 UAC 授权并创建 `RelaEasyTier` 服务及 `%ProgramData%/Rela`。该服务与安装版共享，退出界面仍保持连接。移除前退出所有 Rela，以管理员身份运行包内 `Remove-Network-Service.cmd`，输入 `REMOVE` 后清理服务及其数据，再删除解压目录。共享网络驱动、系统 WebView2 和安装版用户配置会保留。

仅重新打包已编译程序可运行 `npm run package:portable`；`-- --debug` 用于 CI 的 debug 包。发行前应使用完整 `desktop:portable` 构建，保证程序、前端和引擎资产一致。打包使用全新临时目录和固定文件清单，不包含用户 `data` 或构建配置源文件。

### 检查

```powershell
npm run check          # 前端检查、测试、生产构建
npm run format:check
npm run prepare:core   # cargo 检查前先准备固定版本资产
npm run check:rust     # Rust 格式、Clippy、测试
npm run smoke:core     # 两个仅本机通信的 Core；不创建 TUN、不连接外部网络
```

CI 使用不带真实密钥的 debug 构建，不连接实验室网络。生产前端默认只接受 Tauri 运行环境；浏览器生产演示须显式设置 `VITE_RELA_PREVIEW=true`。

## 运行与数据

首次点击连接会安装 `RelaEasyTier` Windows 服务，并写入经过校验的引擎资产。服务独立于界面。最小化或关闭窗口时隐藏到托盘；左键点击托盘图标或选择“打开 Rela”恢复窗口，“退出 Rela”结束界面程序。上述操作均不停止网络连接，需要断开时使用首页开关。目前服务为手动启动，不会随系统自动连接。

| 数据                | 位置                                           |
| ------------------- | ---------------------------------------------- |
| 用户网络配置        | Tauri 应用配置目录的 `network.dat`，DPAPI 加密 |
| 设备名称            | 同目录 `preferences.json`                      |
| 服务程序、Core 配置 | `%ProgramData%/Rela/`，限制为系统和管理员访问  |
| 诊断摘要            | Tauri 应用日志目录，导出后显示完整路径         |

修改配置后，下次连接或“保存并重连”生效。网关地址可留空；未配置时网关和延迟显示未知。未提供资源清单时桌面端返回空列表。

## 目录

```text
config/                  构建默认网络配置
scripts/                 Core 下载校验、隔离连接验证
packaging/portable/      便携包说明、模式标识和服务清理工具
src/                     React 界面、业务服务与浏览器演示
src-tauri/src/commands/   Tauri 业务命令
src-tauri/src/easytier/   服务生命周期、CLI 与 RPC 状态解析
src-tauri/src/platform/   Windows 权限、DPAPI、SCM、网卡和 ICMP
src-tauri/src/network_config.rs  配置校验、存储和 Core TOML
src-tauri/installer/      安装包服务清理
crates/rela-protocol/     共享业务模型
docs/                    产品设计、架构、接口、验收清单
```

详见 [架构](docs/architecture.md)、[接口](docs/protocol.md)、[Windows 验收清单](docs/windows-validation.md)。
