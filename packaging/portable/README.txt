Rela 绿色版 — Windows 64 位

首次使用
1. 将 ZIP 完整解压到本地有写入权限的文件夹，双击 Rela.exe。
   保留包内全部文件；请勿在压缩包中运行，也不要只复制一个 EXE。
2. 如果提示缺少 WebView2，请从微软安装运行库：
   https://developer.microsoft.com/microsoft-edge/webview2/
3. 等待默认配置加载完成，打开“设置 → 网络”，粘贴管理员提供的
   credential 凭据，点击“保存”，然后打开首页的连接开关。
4. 首次连接出现 Windows 管理员授权提示时，确认后继续。
   同一 Windows 用户后续的日常连接、断开和重连通常无需再次授权；
   更新软件或更改网络设置、凭据、设备名称后可能需要重新授权。
5. 等待状态变为“已连接”，再打开“实验室资源”查看可用入口。

日常使用
最小化或关闭窗口会隐藏到系统托盘，点击托盘图标可恢复窗口。
右键托盘图标，选择“退出 Rela”可退出界面。退出界面后连接仍会保持；
需要断开时，请先关闭首页的连接开关。
安装版与绿色版共用本机网络服务，请使用一个 Rela 窗口控制连接。
连接出现问题时，可以查看首页提示或打开“诊断”，将诊断摘要交给管理员。

设置与移动
保留 Rela.exe 同目录的 portable.txt 文件。
本地设置保存在 data 文件夹中。移动文件夹前请从系统托盘退出 Rela。
保存的连接凭据绑定当前 Windows 用户；换电脑或换用户后需要重新填写。
请勿把包含个人设置和凭据的 data 文件夹分享给其他人。

手动更新
从 https://github.com/Starxy/Rela/releases 下载新版完整绿色版 ZIP。
从系统托盘退出 Rela，备份 data 后替换完整程序文件，保留原 data 和 portable.txt。

移除
退出 Rela 后可以删除解压目录，但网络服务会继续保留。
如果需要彻底移除网络服务：
1. 先退出所有 Rela 界面。
2. 右键 Remove-Network-Service.cmd，选择“以管理员身份运行”。
3. 按提示输入 REMOVE。这会断开连接，并影响本机其他 Rela 副本。
4. 确认工具显示完成后，再删除解压目录。

下载与反馈：https://github.com/Starxy/Rela
第三方许可见 THIRD-PARTY-NOTICES.md；checksums.json 提供文件校验值。
