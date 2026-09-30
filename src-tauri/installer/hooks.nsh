; Other Rela copies may share the same executable name. Check only this
; installation directory and ask the user to exit it before manual maintenance.
!ifmacrodef CheckIfAppIsRunning
  !macroundef CheckIfAppIsRunning
!endif
!macro CheckIfAppIsRunning executableName productName
  !define RelaCheckId ${__COUNTER__}
  Push $0
  IfFileExists "$INSTDIR\${executableName}" 0 rela_ready_${RelaCheckId}
  ; GENERIC_WRITE | DELETE, OPEN_EXISTING; this does not modify the file.
  System::Call 'kernel32::CreateFileW(w "$INSTDIR\${executableName}", i 0x40010000, i 7, p 0, i 3, i 0, p 0) p.r0'
  ${If} $0 P<> -1
    System::Call 'kernel32::CloseHandle(p r0)'
  ${Else}
    Pop $0
    SetErrorLevel 2
    Abort "请退出此安装目录中的 ${productName} 后重试。程序文件仍被占用或没有替换权限。"
  ${EndIf}
  rela_ready_${RelaCheckId}:
    Pop $0
  !undef RelaCheckId
!macroend

; The network service and ProgramData configuration are shared across copies.
!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "保留共享的 Rela 网络服务和网络配置。彻底清理服务请使用 Remove-Network-Service.cmd 并确认。"
!macroend
