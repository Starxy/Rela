# Rela

面向实验室成员的远程资源入口。React + TypeScript 提供界面，Tauri 2 提供桌面壳，独立的 `rela-agent` 负责未来的高权限网络操作，EasyTier 负责组网。

依据 [产品与技术设计说明](docs/产品与技术设计说明.md) 初始化。当前是 **0.1.0 开发骨架**，尚未达到文档中的 MVP 完成标准。

## 当前可以做什么

- 浏览器演示：连接 / 断开状态切换、资源搜索、示例诊断、复制与导出摘要、设备偏好保存。
- 桌面壳：400 × 560 紧凑窗口、连接开关和关键状态，资源、设置、详情和诊断使用二级弹窗；支持本地设备名称保存和诊断摘要导出。
- Rust 工作区：桌面端、Agent 入口、共享业务模型及协议版本。
- 工程检查：TypeScript、ESLint、Vitest、Prettier、Rust 测试和 Windows CI。

**真实网络功能尚未实现：** Core 进程管理与 RPC、受限 IPC、Windows Service、自动重连、设备注册、安全凭据存储、实际资源检测与打开、系统托盘及完整日志包。

浏览器预览使用本地示例数据，界面不展示开发提示条，示例资源不会被实际打开。桌面端始终调用 Native，不会自动回退到演示。

## 快速开始

### 浏览器演示

需要 Node.js 22.12+，推荐 Node.js 22 LTS。使用 npm 和仓库中的 `package-lock.json`。

```powershell
npm ci
npm run dev
```

打开 <http://127.0.0.1:1420>。演示偏好保存在浏览器 `localStorage`，连接状态不会跨刷新保存。

### Windows 桌面开发

需要 Rust 1.88+ 的 MSVC 工具链（含 `rustfmt`、`clippy`）、Visual Studio C++ Build Tools、Windows SDK 和 WebView2。详见 [Tauri Windows 前置条件](https://v2.tauri.app/start/prerequisites/#windows)。

```powershell
npm ci
npm run desktop:dev
```

`desktop:dev` 会自行启动前端开发服务器，请先停止占用 1420 端口的 `npm run dev`。

如果本机已有 Rust 但终端找不到 `cargo`，检查 Rust 安装目录是否加入 `PATH`，重新打开终端后再运行。安装标准 rustup 工具链后，可用 `rustup component add rustfmt clippy` 补齐检查组件。

桌面界面初始显示“未连接”；连接命令会返回 `agent_unavailable`，界面显示“后台服务不可用”。设备名称保存在 Tauri 应用配置目录下的 `preferences.json`，诊断摘要写入应用日志目录；导出后界面显示完整路径。

### 构建和检查

```powershell
npm run check          # 代码检查、前端测试、生产构建
npm run format:check   # 前端与文档格式
npm run check:rust     # Rust 格式、Clippy、测试
npm run desktop:build  # 当前桌面壳的 NSIS 安装包
```

生产前端默认只能在 Tauri 中运行，避免发布时误用演示数据。需要检查浏览器生产演示时：

```powershell
$env:VITE_RELA_PREVIEW = 'true'
npm run build
Remove-Item Env:VITE_RELA_PREVIEW
npm run preview
```

Agent 入口可以独立验证：

```powershell
cargo run -p rela-agent -- status
cargo run -p rela-agent -- --version
```

当前 `rela-agent` 是命令行骨架，不会注册 Windows 服务、提升权限或建立网络。

## 目录

```text
src/
  pages/                 主界面、资源和设置弹窗内容
  components/            弹窗、连接详情、资源行与诊断结果
  hooks/                 状态刷新、互斥操作、错误处理
  services/              Tauri 适配器与开发演示
  types.ts               前端业务模型
src-tauri/
  src/commands/          Native 命令
  src/agent/             Agent 客户端边界（待接入传输层）
  src/diagnostics/       离线诊断
  src/easytier/          固定版本与 RPC 约束
  src/credentials/       安全存储接口（待实现）
  src/platform/          Windows 平台接入点
  binaries/              固定版本 Core 放置说明
rela-agent/              独立 Rust 后台服务工程入口
crates/rela-protocol/    共享模型、错误与协议版本
docs/                   产品设计、架构与协议
.github/workflows/      前端检查与 Windows 编译
```

## 后续开发顺序

1. 验证固定版本 EasyTier 2.6.4，补齐 Core 生命周期、RPC 状态读取和 sidecar 资产校验。
2. 接入 Windows Service 和受限 Named Pipe，确保普通用户 GUI 只操作被授权的实验室设备。
3. 实现校园网关和资源探测、系统快捷入口、自动恢复、托盘及日志脱敏。
4. 接入邀请码注册、系统凭据存储、Config Server 和设备撤销。
5. 验证完整安装、升级、卸载流程，完成签名后发布。

Sidecar 分发说明见 [binaries/README.md](src-tauri/binaries/README.md)。当前默认构建不要求下载 Core；单独的 sidecar 配置仅预留分发入口。

详细边界见 [架构说明](docs/architecture.md) 和 [接口协议](docs/protocol.md)。
