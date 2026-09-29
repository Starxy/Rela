# 签名清单与固定入口 v1

本约定落实 T03–T09 的格式部分。客户端业务接口版本、软件版本、资源修订号和清单格式版本分别管理。

## 入口

使用公开仓库 `Starxy/Rela` 的专用 `distribution` 分支；源代码默认分支仍为 `main`。

| 用途           | 固定入口                                                                           |
| -------------- | ---------------------------------------------------------------------------------- |
| 默认网络与资源 | `https://raw.githubusercontent.com/Starxy/Rela/distribution/resources/stable.json` |
| 稳定软件更新   | `https://raw.githubusercontent.com/Starxy/Rela/distribution/software/stable.json`  |
| 测试软件更新   | `https://raw.githubusercontent.com/Starxy/Rela/distribution/software/test.json`    |

资源入口已发布修订 1，客户端匿名读取和验签通过；提交为 `e4de0ab6c494e20e3017e8dc796e1537bd340c07`。两个软件入口尚未发布测试版本。资源更新只修改专用分支，不创建 Release，不影响 GitHub Latest。软件包放在不可覆盖的 `v<version>` Release 下。

## 签名封装

固定入口返回单个 JSON 文件，字段仅允许 `format`、`key_id`、`payload`、`signature`：

- `format` 固定为 `rela.signed.v1`。
- `key_id` 为 1–64 个 ASCII 字母、数字、点、下划线或短横线。
- `payload` 为原始 UTF-8 清单字节的标准 Base64，解码最多 512 KiB；完整封装最多 768 KiB。
- `signature` 为 Ed25519 的 64 字节签名，标准 Base64 编码。

签名消息为 UTF-8 前缀 `Rela signed manifest v1\n<purpose>\n<key_id>\n` 加解码后的原始 payload。purpose 为 `resources` 或 `software`，同一签名不能跨用途使用。先验签再解析，不对 JSON 重新排序或重新序列化；一个封装同时包含内容和签名，避免分开上传时读到不匹配版本。

客户端只信任 `config/distribution.json` 中嵌入的公钥，按 key_id 和 purpose 精确匹配。测试 fixture 的密钥只用于测试，不能加入生产信任配置。生产私钥存于受保护发布环境，本机样本签名工具以当前用户 DPAPI 保存私钥；正式启用前还需离线备份和发布端配置。

轮换时先发布同时信任旧、新公钥的客户端，再用新密钥签名；旧客户端需先升级。等迁移完成后，在后续客户端中移除旧公钥。远程清单不能自行为客户端新增信任根。资源/软件清单密钥分别生成；Tauri 软件包签名另用官方 updater 密钥，算法格式按 [Tauri 官方文档](https://v2.tauri.app/plugin/updater/) 配置。

## 资源 payload

样本见 [resources.example.json](../config/resources.example.json)。严格字段：

- `schema_version: 1`。
- `version` 为 1 至 9007199254740991 的递增整数。
- `network` 为 1–128 字符网络名，首尾无空白，无控制字符。
- `peer` 为 1–16 个不重复的合法 tcp/udp/quic/ws/wss 地址，无账号、密码、查询或片段。
- `resources` 最多 256 项，每项必须有稳定 `id`、`name`、`kind`、`address`、`description`；`port` 和 `username` 仅在支持的类型中使用。

| 类型 | 地址与参数                                                                             | 探测              |
| ---- | -------------------------------------------------------------------------------------- | ----------------- |
| ssh  | 主机/IP，必填 1–65535 端口，可选安全字符用户名；无任意选项或命令                       | 目标 TCP 端口     |
| web  | http/https URL，不允许 URL 用户名或密码；可选端口必须与 URL 一致                       | URL 端口或 80/443 |
| nas  | `\\host\share\path`，禁止设备路径、越界段、ADS 和非法 Windows 字符；可选端口只能是 445 | 主机 TCP 445      |

禁止额外字段，包括 credential、network_secret、private_mode、disable_p2p、gateway_ip、设备偏好及 availability。资源 ID 不重复；整个清单通过后才保存和应用。资源可用性由客户端探测生成。

已验证资源 payload 的 SHA-256 与修订号一起缓存。低于缓存版本拒绝；相同版本只接受相同原始内容。回退内容必须使用更高修订号重新签名发布。更换 JSON 排版也需要新修订号。

## 软件 payload

严格字段：`schema_version: 1`、递增整数 `revision`、SemVer `version`、`channel`（stable/test）、`target: windows-x86_64`、`notes`、`published_at`、`core_version`、`minimum_app_version`、`installer`、`portable`。

`installer` 和 `portable` 都包含 `url`、`size`（1 字节至 2 GiB）、小写十六进制 `sha256`、Tauri/minisign `signature`。地址必须固定到 `https://github.com/Starxy/Rela/releases/download/v<version>/<文件名>`，安装包为 exe，绿色包为 zip，不接受 latest、其他仓库、跳转参数或可变路径。

稳定渠道拒绝预发布版本；测试渠道独立入口。只向更高 SemVer 更新，不自动降级；低于 minimum_app_version 提示先手动升级。revision 与软件版本独立，用于拒绝过时的软件元数据。资源修订号不影响软件版本比较。

Cargo 工作区、package.json、package-lock.json、tauri.conf.json 和 Release tag 的版本必须一致，发布前由 `scripts/check-versions.mjs` 检查。Core 保持固定版本，随对应软件版本发布，不跟随 latest。

## 软件包下载与绿色更新包

下载器按签名软件清单给定的大小写入临时文件，限定 HTTPS 主机、跳转与超时；只接受完整 200 响应，拒绝部分响应、过量数据和中断。落盘后逐块核对 SHA-256 和独立的 Tauri/minisign 包签名。验证后的 Windows 文件句柄保持禁止写入/删除；重新打开必须重新校验。失败或正常释放时清除本次临时文件，不修改原程序。

`package:portable` 同时生成首次手动安装 ZIP 和 `Rela_<version>_x64-update.zip`。更新 ZIP 的单一根目录为 `Rela_<version>_x64-update/`，包含 15 个固定程序/许可文件及 `checksums.json`；不含 `portable.txt`、`data`、凭据或本地配置。手动安装 ZIP 仍保留标记文件。Debug 包带 `-debug` 文件名和 `profile: debug`，运行时拒绝作为正式更新使用。

更新包 `checksums.json` 使用 schema_version=1、version、target=windows-x86_64、profile=release、core_version、engine_revision 及逐文件 SHA-256 的 files 映射。版本与 Core 信息必须匹配软件清单，Core 自身资产清单必须与逐文件摘要一致。解压前拒绝路径越界、反斜杠/ADS/设备名、未知文件、大小写重复、链接、加密条目和超限内容；只向全新目录写入，逐文件校验并检查主程序、Core、CLI 的 PE x64 头。暂存前检查可用空间，失败只清理本次创建的暂存内容。

2026-09-29：下载、独立包签名和安全暂存模块已有 7 项测试，真实 loopback HTTP 及签名 ZIP 测试通过；已生成并检查本地 debug 更新 ZIP。已接入绿色版确认、进度、助手交接和启动恢复；独立 minisign 包签名密钥已生成并完成小型本地样本签名复验，私钥在仓库外以 DPAPI 保存。官方安装器、发布工作流和真实升级验收继续实现。此阶段不代表 T37、T42 或真实升级验收已完成。

安装版适配器进一步限制安装包不超过 512 MiB，并要求包签名的 trusted comment 包含唯一、匹配清单的 `version:<SemVer>` 字段。官方 updater 的本机 JSON 从已验签软件清单即时派生，不单独发布或信任第二份远程版本元数据；详见本体更新设计。Tauri 配置已开启 createUpdaterArtifacts，公钥必须与 config/package-signing.json 一致，版本检查脚本会核对这些约束。

本地 package-tool 的 `sign` 使用工作区版本，`sign-version` 允许显式指定测试版本；两者签名后都用生产校验器复验版本、摘要和签名。`bundle <仓库外 DPAPI 私钥文件>` 为已经完成的 release 构建调用固定 Tauri 打包器，密钥仅进入该子进程环境，生成 NSIS 签名后再次独立校验；它不执行安装器或发布资产。私钥备份及受保护 CI 托管仍待完成。
