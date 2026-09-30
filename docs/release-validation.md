# 发布产物验证

## 产物范围

发行仅包含 NSIS 安装包与完整 Portable ZIP。绿色包直接复制编译得到的程序，Tauri 为安装包写入自身的包类型标记，两份主程序不要求字节一致。引擎、许可证与服务清理工具等共享资源必须一致。

## 日常验证顺序

1. 程序源码变化时构建最新程序与前端；仅修改打包脚本时可复用编译结果。
2. 从同一构建生成 NSIS 安装包和完整绿色版 ZIP。
3. 运行 `npm run check:packages`；debug 使用 `npm run check:packages -- --debug`。需要 7-Zip 在 PATH。
4. 在独立 Windows 测试机验证手动安装、手动替换、连接和卸载。

`check:packages` 检查压缩内容、固定文件清单、敏感内容、Core 固定摘要，以及两包共享资源的一致性；绿色版主程序须与编译输出相同。报告保存在 `target/<profile>/bundle/validation.json`。检查不执行安装器，不能证明真实安装或网络连接成功。

Windows CI 执行工具测试、NSIS 构建与两包检查，无需软件更新包私钥。

## 隔离构建

`npm run build:clean` 在新目录收集允许的输入、限制继承环境、注入合成本机配置探针，再完成 release 构建、打包和扫描；`-- --debug` 使用 debug。输入清单、日志和产物报告保存在 `target/clean-release/<session>`。`-- --prepare-only` 仅准备输入，不能视为构建通过。

该流程使用本机工具链与缓存。只有对应报告的 `complete` 和 `passed` 都为 true 才表示本次完整构建通过；增量构建或旧报告不能替代它。

## 主流程实测

验证两种发行形式的首次使用、credential、真实连接和资源访问；用较新软件清单检查提示与 GitHub Release 跳转，再手动安装或替换程序，确认配置保留和网络服务可用。详见 [Windows 验收](windows-validation.md)。
