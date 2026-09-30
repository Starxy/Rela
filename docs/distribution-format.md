# 资源配置、软件清单与固定入口

资源配置为普通 JSON v1，软件版本清单为签名 v2。客户端业务接口版本、软件版本、资源展示版本和清单格式版本分别管理。

## 入口

资源与网络配置只维护在公开仓库 `Starxy/Rela` 的 `main` 根目录 [resources.json](../resources.json)。启动时在后台静默读取，手动点击“更新线上配置”时读取同一地址。

| 用途           | 固定入口                                                                             |
| -------------- | ------------------------------------------------------------------------------------ |
| 默认网络与资源 | `https://raw.githubusercontent.com/Starxy/Rela/refs/heads/main/resources.json`       |
| 稳定软件更新   | `https://raw.githubusercontent.com/Starxy/Rela/refs/heads/main/software/stable.json` |
| 测试软件更新   | `https://raw.githubusercontent.com/Starxy/Rela/refs/heads/main/software/test.json`   |

资源更新直接编辑并提交根目录文件，不需要签名、独立分支或 Release。两个软件入口仍为预留路径，尚未发布测试版本。软件包放在不可覆盖的 `v<version>` Release 下。

## 软件签名封装

软件入口返回单个 JSON 文件，字段仅允许 `format`、`key_id`、`payload`、`signature`：

- `format` 固定为 `rela.signed.v1`。
- `key_id` 为 1–64 个 ASCII 字母、数字、点、下划线或短横线。
- `payload` 为原始 UTF-8 清单字节的标准 Base64，解码最多 512 KiB；完整封装最多 768 KiB。
- `signature` 为 Ed25519 的 64 字节签名，标准 Base64 编码。

签名消息为 UTF-8 前缀 `Rela signed manifest v1\nsoftware\n<key_id>\n` 加解码后的原始 payload。先验签再解析，不对 JSON 重新排序或重新序列化；一个封装同时包含内容和签名。旧资源缓存保留原 `resources` 用途验签兼容，新下载的资源只接受普通 JSON。

客户端只信任 `config/distribution.json` 中嵌入的公钥，按 key_id 和 purpose 精确匹配。测试 fixture 的密钥只用于测试，不能加入生产信任配置。生产私钥存于受保护发布环境，本机样本签名工具以当前用户 DPAPI 保存私钥；正式启用前还需离线备份和发布端配置。

软件密钥轮换时先发布同时信任旧、新公钥的客户端，再用新密钥签名；旧客户端需先手动升级。迁移完成后再移除旧公钥。远程清单不能自行新增信任根。原资源公钥只用于读取旧离线缓存。

## resources.json

生产配置见 [resources.json](../resources.json)，测试样本见 [resources.example.json](../config/resources.example.json)。文件最多 512 KiB，严格字段：

- `schema_version: 1`。
- `version` 为 1 至 9007199254740991 的整数，仅作为展示信息；更新内容不要求递增。
- `network` 为 1–128 字符网络名，首尾无空白，无控制字符。
- `peer` 为 1–16 个不重复的合法 tcp/udp/quic/ws/wss 地址，无账号、密码、查询或片段。
- `resources` 最多 256 项，每项必须有稳定 `id`、`name`、`kind`、`address`、`description`；`port` 和 `username` 仅在支持的类型中使用。

| 类型 | 地址与参数                                                                             | 探测              |
| ---- | -------------------------------------------------------------------------------------- | ----------------- |
| ssh  | 主机/IP，必填 1–65535 端口，可选安全字符用户名；无任意选项或命令                       | 目标 TCP 端口     |
| web  | http/https URL，不允许 URL 用户名或密码；可选端口必须与 URL 一致                       | URL 端口或 80/443 |
| nas  | `\\host\share\path`，禁止设备路径、越界段、ADS 和非法 Windows 字符；可选端口只能是 445 | 主机 TCP 445      |

禁止额外字段，包括 credential、network_secret、private_mode、disable_p2p、gateway_ip、设备偏好及 availability。资源 ID 不重复；整个清单通过后才保存和应用。资源可用性由客户端探测生成。

客户端以固定地址返回的最新有效内容为准；相同或较低 `version` 的合法配置也可应用。SHA-256 用于检查本地缓存完整性。旧版已验签缓存仍可离线读取，成功获取新配置后写入普通 JSON 缓存，并弃用旧入口的 ETag。

本地覆盖 network/peer 后暂停整个资源配置自动获取，重启后保持。手动刷新校验并保存成功后恢复线上值和自动获取；网络或格式错误保留有效缓存、本地修改和覆盖状态。本机开关、网关、设备名称与 credential 不由远程覆盖；network 改名仍须满足凭据绑定校验。刷新不自动重连。

先运行 `cargo build -p rela-manifests --bin rela-manifest` 准备校验工具，再运行 `node scripts/publish-resources.mjs` 校验根文件；附加 `--publish` 可将已验证文件写入 `main/resources.json`。该工具不需要资源签名密钥。

## 软件 payload v2

仅允许以下字段：

```json
{
  "schema_version": 2,
  "revision": 1,
  "version": "0.2.0",
  "channel": "stable",
  "target": "windows-x86_64",
  "notes": "版本说明",
  "published_at": "2026-09-30T00:00:00Z"
}
```

以上仅为格式示例，不代表该软件版本已发布。稳定渠道拒绝预发布版本，测试渠道独立入口；只提示高于当前版本的 SemVer，拒绝 build metadata。revision 为递增整数，与资源修订及软件版本独立。同一 revision 的原始内容不可改变；修改说明需提高 revision。

客户端通过固定仓库与版本生成 `https://github.com/Starxy/Rela/releases/tag/v<version>`。清单不接受自定义跳转地址、安装包信息或最低自动升级版本。打开页面前重新核对已验签缓存与用户看到的版本，用户自行下载、安装或替换。

两个软件入口尚未发布正式软件清单，应按 v2 格式发布。旧 v1 软件清单不再支持。先确保对应 Release 与安装包、绿色版 ZIP 可匿名获取，再发布所选渠道的签名清单。

Cargo 工作区、package.json、package-lock.json、tauri.conf.json 和 Release tag 的版本须一致，由 `scripts/check-versions.mjs` 检查。Core 保持固定版本并随对应软件包发布。

## 发布产物

只构建 NSIS 安装包和完整 Portable ZIP，后者包含 `portable.txt`、使用说明、固定引擎资产和逐文件 SHA-256 校验表。包内不含用户 data、凭据或开发者配置。打包与扫描规则见[产物验证](release-validation.md)。

软件检查仅下载签名版本清单；应用内包下载、包验签、更新 ZIP、安装助手和软件恢复事务均已移除。
