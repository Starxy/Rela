Rela Portable — Windows x64 免安装版

使用
1. 将 ZIP 完整解压到有写入权限的本地文件夹（推荐 NTFS），双击 Rela.exe。
   请勿在压缩包内直接运行，也不要只复制一个 EXE。
2. 电脑需要 Microsoft Edge WebView2 Runtime；若启动提示缺少运行库，可从微软安装：
   https://developer.microsoft.com/microsoft-edge/webview2/
3. 连接、断开和重连需要同意 Windows UAC 授权。
4. 最小化或关闭窗口会隐藏到托盘。点击托盘图标恢复，右键选择“退出 Rela”退出界面。
   退出界面后网络连接继续保持；需要断开时请使用首页开关。

数据和网络服务
portable.txt 是便携模式标识，请保留在 Rela.exe 同目录。
界面设置、加密网络配置、诊断和 WebView2 缓存保存在同目录的 data 文件夹。
移动文件夹前请从托盘退出。已保存网络密钥受当前 Windows 用户的 DPAPI 保护，
换电脑或换用户后请在设置中恢复默认或重新填写配置；data 不应随发行包分发。

本版省去 Rela 安装程序。首次连接仍会注册专用 Windows 服务 RelaEasyTier，
并将引擎及运行配置写入 %ProgramData%\Rela，网络驱动由 Windows 管理。
因此删除解压目录不会自动停止或移除网络服务。
安装版和 Portable 版共用这一服务，请勿同时使用多个副本控制连接。
只读目录、FAT/exFAT 移动盘无法保证权限保护，请使用本地 NTFS 文件夹。

移除
1. 先退出所有 Rela 界面。
2. 右键 Remove-Network-Service.cmd，选择“以管理员身份运行”。
   输入 REMOVE 后移除 RelaEasyTier 服务及其 ProgramData 数据。
   这会断开 Rela 网络，也会影响本机安装版 Rela 使用的同一个服务。
3. 确认工具显示完成后，删除本文件夹即可移除便携界面及本地设置。
该工具不会卸载系统 WebView2 或共享网络驱动，也不会清理安装版的用户配置。

本包为未签名的内部测试版本。许可说明见 THIRD-PARTY-NOTICES.md。
checksums.json 提供文件 SHA-256 校验值。
