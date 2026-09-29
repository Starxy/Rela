# Rela 架构

## 调用关系

```text
React / TypeScript
       ↓ Tauri 业务命令
Rela Rust Native
       ├─ 网络配置校验、DPAPI 存储、诊断
       ├─ 同一 Rela 程序的临时提权入口 → Windows SCM
       └─ 官方 easytier-cli → 127.0.0.1:35888 RPC
                                      ↓
                        EasyTier Core 2.7.0-0a783c8e Windows 服务
```

不开发独立 Agent 或服务端。后台网络由专用 `RelaEasyTier` 服务承载。GUI 不需要长期管理员权限；连接、断开、重连目前都通过 UAC 授权。同一程序的 helper 只执行本次服务操作并退出。

## 配置与服务

构建脚本只从 `config/network.default.json` 选取公共字段，不读取开发者本地 JSON、DPAPI 凭据或 `RELA_NETWORK_SECRET`，发布构建无需密钥。当前公共默认值仍内置；远程资源清单属于后续阶段。

网络配置及 credential 保存为当前用户的 DPAPI 文件 `network.dat`。读取接口只返回 `has_credential`；省略更新字段表示保留同网络凭据，空字符串表示清除。网络名变更不能复用未重新提交的旧凭据。恢复默认清除已存凭据。旧版配置只恢复公共字段，重新导入并保存后覆盖旧密码；后端没有密码认证回退。

提权请求包含有限的动作枚举和已验证配置，以机器范围 DPAPI 加密，临时文件 ACL 仅允许创建者、管理员和系统读取，操作后删除。helper 再次验证输入，不接受任意程序路径或 CLI 参数。

首次连接将固定版本、经过 SHA-256 校验的 Core 和运行库复制到 `%ProgramData%/Rela/engine`，生成受保护的 `core.toml` 并用官方 CLI 安装手动启动服务。再次连接使用 Windows SCM 启停。Core 使用 Secure Mode，`local_private_key` 为获授权的 credential，`local_public_key` 由 X25519 推导；不写入 `network_secret` 或 `peer_public_key`，启动参数只包含配置路径及固定选项。配置保留固定实例 ID、DHCP、TUN 名称 `Rela`、无监听节点、回环 RPC、禁用环境变量解析和用户选择的两个网络开关。

服务控制使用进程内互斥和受保护目录中的独占文件锁，防止多个 Rela 实例同时更新服务配置或引擎。服务名称、程序路径、资源哈希固定；同名但指向其他程序的服务会被拒绝。引擎与配置以临时文件原子替换。

服务独立于 GUI，最小化或关闭主窗口会隐藏到托盘，不调用断开。Native 使用 Tauri TrayIcon 创建托盘图标；左键点击或菜单“打开 Rela”取消最小化、显示并聚焦原窗口；菜单“退出 Rela”直接退出界面进程。仅在托盘创建成功后拦截窗口关闭与最小化事件。当前未配置开机启动、SCM 失败恢复或 Rela 自动恢复。卸载会停止并删除专用服务，保留本地配置目录；升级行为仍需在独立机器验收。

## Portable 分发与存储

`desktop:portable` 先执行 Tauri 无安装器构建，再将 EXE、固定引擎资产、许可和清理工具打包为 ZIP。每次从新临时目录开始，仅复制明确列出的文件，并生成包内文件校验表与 ZIP SHA-256。CI 使用不带真实密钥的 debug 包验证打包流程。

`portable.txt` 位于 EXE 同目录时，`AppPaths` 将配置、诊断和 WebView2 数据目录分别指向该目录下的 `data/config`、`data/logs`、`data/webview`，路径与当前工作目录无关。主窗口在 setup 中构建，以便 WebView2 创建前设置数据目录。安装版继续使用 Tauri 默认用户目录。DPAPI 绑定当前 Windows 用户，数据目录不能作为跨机器密钥迁移方案。

网络服务生命周期保持一致：Portable 首次连接仍创建 `RelaEasyTier`，其受保护工作目录固定在 ProgramData，与安装版共用。包内清理工具要求管理员权限和输入确认，核对服务程序路径、等待服务停止与删除，拒绝目录中的重解析点后再清理服务数据。共享驱动和系统 WebView2 不由该工具卸载。

## 状态与诊断

每五秒刷新业务状态。Native 调用官方 CLI 查询 `node`、`connector` 和按需查询 `route`，单次命令设置超时并限制输出大小。Core 返回的完整配置可能包含密钥，解析类型只取业务所需字段，不向前端或诊断导出原始 JSON。

已连接要求：服务运行、实例 ID 匹配、至少一个节点连接成功、有虚拟 IPv4、系统中对应 TUN 确实拥有该地址。未获取状态时清除旧的已连接显示。

配置校园网关后通过 ICMP 检测延迟，路由表用于判断直连或中继。ICMP 失败只代表探测失败，不能推断所有业务端口不可用。资源由 Rela 单独管理，不能从 Peer 数推算可用资源数量。

## 安全边界与限制

- 前端没有 shell、文件系统插件权限。CSP 禁止远程脚本和页面。
- 安装包不包含网络主密码或客户端 credential。DPAPI 绑定当前 Windows 用户；同一用户下运行的程序以及管理员不在其隔离边界内。
- Core 工作配置是管理员受保护目录中的明文 TOML；管理员和系统能够读取。
- RPC 使用回环监听及地址白名单，目前没有独立的调用者身份认证；本机其他进程可能访问 Core RPC。这是当前接入方式的限制。
- 诊断只导出白名单业务字段，不打包原始日志。摘要包含虚拟 IP 和本机导出路径。
- Windows x64 是本期平台，其他平台的系统控制接口明确返回不支持。

## 验证范围

前端测试覆盖命令映射、凭据格式、网络绑定与清除；Rust 测试覆盖 TOML credential 身份、X25519 标准向量、旧配置读取、DPAPI、凭据不回显、RPC、Windows 参数及文件互斥。新 CLI 的 protobuf JSON 会省略值为零的已连接状态，非零状态使用枚举字符串；解析要求有效的节点 URL，并拒绝把未知状态当作已连接。

隔离测试验证 CLI/TOML/附加启动参数认证与错误凭据拒绝；现网测试使用实际配置生成代码验证 lab201 的 TOML 认证、地址分配和固定实例 ID。测试关闭 TUN，临时明文文件受 ACL 保护并在退出时清理。

隔离验证使用 loopback 和 `no_tun`，不替代服务安装、UAC、驱动、真实网关和资源访问验收。详见 [Windows 验收清单](windows-validation.md)。

## 工程约定

Node.js 22.12+，Rust 1.88+，提交 npm 与 Cargo 锁文件。Tauri 前端 API、Rust crate、CLI 使用 2.11.x。EasyTier 固定官方开发版 `2.7.0-0a783c8e`，完整提交和 SHA-256 见 `config/easytier-version.json`；Native 直接嵌入该受版本管理的清单进行资产验证，不在运行时下载 latest。Actions 产物有保留期限，长期归档仍待落实。业务模型修改须同步 Rust、TypeScript、接口文档和相关行为测试。
