# EasyTier 资产

运行 `npm run prepare:core` 获取并校验 [EasyTier v2.6.4](https://github.com/EasyTier/EasyTier/releases/tag/v2.6.4) Windows x64 官方发行包。下载来源和固定 SHA-256 记录在 `scripts/prepare-easytier.mjs`。

生成的 `easytier/` 目录被 Git 忽略，包含 Core、CLI、wintun.dll、Packet.dll、WinDivert64.sys 和 manifest.json。manifest 在编译时嵌入 Native，运行时校验文件。安装包通过 Tauri resources 将整个目录放在 `easytier/` 下。

Cargo 编译、测试前必须先准备资产；desktop:dev / desktop:build 自动执行准备。隔离验证使用 `npm run smoke:core`。

许可证和来源见仓库根目录 THIRD-PARTY-NOTICES.md；安装包携带该文件和 third-party-licenses。Packet.dll 的再分发条件是正式发布前的待解决项。
