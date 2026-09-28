# EasyTier 二进制文件

目标版本固定为 **2.6.4**，来自 [EasyTier 官方 Release](https://github.com/EasyTier/EasyTier/releases/tag/v2.6.4)。这是集成目标，尚未通过 Rela 的组网验证。

初始化工程不包含或自动下载 Core，也不会以 GUI 权限启动 Core。

后续集成要求：

1. 从官方固定版本获取目标架构文件，记录来源、SHA-256、许可证及所需 Windows 驱动文件。
2. 在测试环境验证签名、启动参数、RPC 回环监听和网络行为。
3. 按 Tauri target triple 命名，例如 `easytier-core-x86_64-pc-windows-msvc.exe`，放入本目录。
4. 服务集成完成后，用 `npm run tauri -- build --config src-tauri/tauri.sidecar.conf.json` 将 Core 纳入安装包。该配置只负责携带文件；服务安装、生命周期管理和驱动分发还需实现。

默认配置不引用不存在的 sidecar，以便空仓库可以独立编译桌面壳。不要使用运行时下载 `latest`，也不要把真实网络配置、设备凭据或共享密钥提交到仓库。

命名约定见 [Tauri External Binaries 文档](https://v2.tauri.app/develop/sidecar/)。
