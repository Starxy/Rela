# 签名清单与固定入口

资源清单为 v1，软件版本清单为 v2，签名封装仍为 rela.signed.v1。客户端业务接口版本、软件版本、资源修订号和清单格式版本分别管理。

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

轮换时先发布同时信任旧、新公钥的客户端，再用新密钥签名；旧客户端需先手动升级。迁移完成后再移除旧公钥。远程清单不能自行新增信任根，资源和软件清单使用独立用途密钥。

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

两个软件入口尚未发布正式软件清单，应按 v2 格式发布。旧 v1 软件清单不再支持；资源 v1 和签名封装保持兼容。先确保对应 Release 与安装包、绿色版 ZIP 可匿名获取，再发布所选渠道的签名清单。

Cargo 工作区、package.json、package-lock.json、tauri.conf.json 和 Release tag 的版本须一致，由 `scripts/check-versions.mjs` 检查。Core 保持固定版本并随对应软件包发布。

## 发布产物

只构建 NSIS 安装包和完整 Portable ZIP，后者包含 `portable.txt`、使用说明、固定引擎资产和逐文件 SHA-256 校验表。包内不含用户 data、凭据或开发者配置。打包与扫描规则见[产物验证](release-validation.md)。

客户端仅下载签名版本清单；应用内包下载、包验签、更新 ZIP、安装助手和软件恢复事务均已移除。
