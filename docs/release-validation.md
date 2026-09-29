# 发布产物验证

## 共用程序一致性

安装版自动更新使用签名更新 ZIP 中的摘要核对 NSIS 助手和安装后的程序文件。Tauri CLI 2.11.5 在 NSIS 打包时将 EXE 内的 bundle type 从 UNK 改为 NSS，完成后恢复原始 EXE；直接把原始文件装进 ZIP 会导致正常更新被拒绝。

`package-portable.mjs` 现在只在副本上应用同一标记变化，写入 `target/<profile>/bundle/payload/Rela.exe`，并用于初装 ZIP 与更新 ZIP。应用仍通过 `portable.txt` 选择便携模式。原始编译文件不改动，客户端验签与摘要校验不放宽。当前流程使用 minisign 包签名；已附加 Authenticode 的输入会拒绝处理，以免破坏证书。

## 日常验证顺序

1. 程序源码变化时构建一次最新程序与前端；仅修改打包脚本时可先复用已有编译结果验证。
2. 从同一构建生成 NSIS 安装包和两种 Portable ZIP。
3. 运行 `npm run check:packages`；debug 产物使用 `npm run check:packages -- --debug`。需要 7-Zip 在 PATH 中。
4. 包签名后使用独立校验器确认签名、摘要与版本，再进入独立 Windows 实测。

`check:packages` 检查压缩内容、固定文件清单、敏感内容规则、Core 固定摘要，以及三个包中全部共用文件的一致性。报告在 `target/<profile>/bundle/validation.json`。该报告不证明包签名有效，也不证明实际安装与升级成功；两者分别验证。

Windows CI 已加入工具测试、无签名 NSIS 构建及三包比对，无需发布私钥。CI 不执行安装器或连接实验室网络。

## 隔离构建

`npm run build:clean` 在新目录中收集允许的输入、限制继承环境，注入合成本机配置探针，再完成 release 构建、打包和扫描；`-- --debug` 使用 debug。输入清单、日志检查及产物报告保留在 `target/clean-release/<session>`。`-- --prepare-only` 只准备输入，不能视为构建通过。此流程使用本机工具链和缓存，不是完全隔离的安全沙箱。

只有对应报告的 `complete` 和 `passed` 都为 true 才表示这次隔离构建通过。普通增量构建、部分扫描通过或旧报告均不能替代 T43 的完整验收。

## 主流程实测

下一步在独立 Windows 环境验证：手动安装首个具备更新能力的版本，填写 credential 并连接，访问资源；随后安装版和绿色版分别完成一次连续签名版本升级，核对原配置保留、新 GUI/Core 版本和网络状态。保留签名错误、UAC 取消及配置恢复检查，复杂组合中断测试放在正常流程通过后。具体清单见 [Windows 验收](windows-validation.md)。
