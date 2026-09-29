# EasyTier 资产

运行 `npm run prepare:core` 获取并校验 [EasyTier 官方 Windows x64 开发构建](https://github.com/EasyTier/EasyTier/actions/runs/35748842247)，版本固定为 `2.7.0-0a783c8e`。提交、来源、ZIP 和每个文件的 SHA-256 记录在 `config/easytier-version.json`，不会自动追随 latest。

首次下载需要安装并登录 GitHub CLI（`gh auth login`），CI 使用 `GH_TOKEN`。官方 Actions 产物当前保留至 2026-12-21；下载后应保留 `target/easytier-reference/easytier-windows-x86_64-2.7.0-0a783c8e.zip`。离线或上游产物过期后可设置 `RELA_CORE_ARCHIVE` 指向该原始 ZIP，校验仍须完全一致；后续长期归档与发布渠道见 todo T11。不要重新压缩文件后冒充原始归档。

生成的 `easytier/` 目录被 Git 忽略，包含 Core、CLI、wintun.dll、Packet.dll、WinDivert64.sys 和 manifest.json。Native 从受版本管理的 `config/easytier-version.json` 嵌入校验值，运行时校验文件。安装包通过 Tauri resources 将整个目录放在 `easytier/` 下。

desktop:dev / desktop:build 自动准备资产。隔离验证使用 `npm run smoke:core` 和 `npm run test:credential`。

许可证和来源见仓库根目录 THIRD-PARTY-NOTICES.md；安装包携带该文件和 third-party-licenses。Packet.dll 的再分发条件是正式发布前的待解决项。
