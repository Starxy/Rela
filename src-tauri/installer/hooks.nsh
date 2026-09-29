; The elevated bootstrap relies on 3.11's restricted PLUGINSDIR creation.
; Review a new NSIS release before changing this audited compiler pin.
!if "${NSIS_VERSION}" != "v3.11"
  !error "Rela requires the reviewed NSIS 3.11 bootstrap."
!endif

Var RelaUpdateRequest
Var RelaHelperFile
Var RelaHelperDirectory
Var RelaInstallerPid
Var RelaHookResult

; Never execute an installed/user-selected helper with administrator privileges.
; This executable comes from the signed installer itself. Keep its temporary
; directory anchored and its bytes locked across both synchronous hook calls.
!macro RelaPrepareHelper
  InitPluginsDir
  Push $0
  SetOutPath "$PLUGINSDIR"
  System::Call 'kernel32::CreateFileW(w "$PLUGINSDIR", i 0x80000000, i 3, p 0, i 3, i 0x02000000, p 0) p.r0'
  StrCpy $RelaHelperDirectory $0
  ${If} $RelaHelperDirectory P= -1
    Pop $0
    SetErrorLevel 2
    Abort "无法保护安装助手目录，安装尚未开始。"
  ${EndIf}
  ClearErrors
  File /oname=RelaUpdate.exe "${MAINBINARYSRCPATH}"
  ${If} ${Errors}
    System::Call 'kernel32::CloseHandle(p $RelaHelperDirectory)'
    Pop $0
    SetErrorLevel 2
    Abort "无法准备安装助手，安装尚未开始。"
  ${EndIf}
  System::Call 'kernel32::CreateFileW(w "$PLUGINSDIR\RelaUpdate.exe", i 0x80000000, i 1, p 0, i 3, i 0, p 0) p.r0'
  StrCpy $RelaHelperFile $0
  ${If} $RelaHelperFile P= -1
    System::Call 'kernel32::CloseHandle(p $RelaHelperDirectory)'
    Pop $0
    SetErrorLevel 2
    Abort "无法锁定安装助手，安装尚未开始。"
  ${EndIf}
  System::Call 'kernel32::GetCurrentProcessId() i.r0'
  StrCpy $RelaInstallerPid $0
  SetOutPath $INSTDIR
  Pop $0
!macroend

!macro RelaReleaseHelper
  System::Call 'kernel32::CloseHandle(p $RelaHelperFile)'
  System::Call 'kernel32::CloseHandle(p $RelaHelperDirectory)'
!macroend

!macro RelaRefusePending
  !define RelaPendingId ${__COUNTER__}
  IfFileExists "$INSTDIR\.rela-installed-control" rela_pending_${RelaPendingId}
  IfFileExists "$INSTDIR\.rela-installed-update" rela_pending_${RelaPendingId} rela_clear_${RelaPendingId}
  rela_pending_${RelaPendingId}:
    SetErrorLevel 2
    Abort "此安装目录仍有未完成的更新。请先从发起更新的 Rela 恢复，再安装或卸载。"
  rela_clear_${RelaPendingId}:
  !undef RelaPendingId
!macroend

!macro NSIS_HOOK_PREINSTALL
  Push $0
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/RELAUPDATE=" $RelaUpdateRequest
  ${If} ${Errors}
    StrCpy $RelaUpdateRequest ""
  ${EndIf}
  Pop $0
  ${If} $RelaUpdateRequest != ""
    ${If} $UpdateMode != 1
    ${OrIf} $WixMode != 0
      SetErrorLevel 2
      Abort "更新请求不匹配受支持的安装模式。"
    ${EndIf}
  ${Else}
    !insertmacro RelaRefusePending
  ${EndIf}
  !insertmacro RelaPrepareHelper
  ${If} $RelaUpdateRequest != ""
    ClearErrors
    ExecWait '"$PLUGINSDIR\RelaUpdate.exe" --rela-nsis-pre "$RelaUpdateRequest" "$INSTDIR" "$RelaInstallerPid"' $RelaHookResult
    ${If} ${Errors}
    ${OrIf} $RelaHookResult != 0
      !insertmacro RelaReleaseHelper
      SetErrorLevel 2
      ; Automatic update has an independent controller which reports/retries
      ; recovery. Exit now so it can prove NSIS is no longer writing files.
      Quit
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ClearErrors
  ${If} $RelaUpdateRequest != ""
    ExecWait '"$PLUGINSDIR\RelaUpdate.exe" --rela-nsis-post "$RelaUpdateRequest" "$INSTDIR" "$RelaInstallerPid"' $RelaHookResult
  ${Else}
    ExecWait '"$PLUGINSDIR\RelaUpdate.exe" --rela-nsis-maintenance "$INSTDIR"' $RelaHookResult
  ${EndIf}
  ${If} ${Errors}
    StrCpy $RelaHookResult 2
  ${EndIf}
  !insertmacro RelaReleaseHelper
  ${If} $RelaHookResult != 0
    SetErrorLevel 2
    ${If} $RelaUpdateRequest != ""
      Quit
    ${EndIf}
    Abort "安装确认未完成。请从 Rela 重试恢复；备份和配置不会自动删除。"
  ${EndIf}
!macroend

; The stock Tauri macro kills every process with the same image name. Rela
; installations and Portable copies share that name, so only probe this target.
; No process is terminated by an installer/uninstaller.
!ifmacrodef CheckIfAppIsRunning
  !macroundef CheckIfAppIsRunning
!endif
!macro CheckIfAppIsRunning executableName productName
  !define RelaCheckId ${__COUNTER__}
  Push $0
  Push $1
  StrCpy $1 0
  rela_wait_${RelaCheckId}:
    IfFileExists "$INSTDIR\${executableName}" 0 rela_ready_${RelaCheckId}
    ; GENERIC_WRITE | DELETE, OPEN_EXISTING; this does not modify the file.
    ; A running image or handle denying replacement makes the probe fail.
    System::Call 'kernel32::CreateFileW(w "$INSTDIR\${executableName}", i 0x40010000, i 7, p 0, i 3, i 0, p 0) p.r0'
    ${If} $0 P<> -1
      System::Call 'kernel32::CloseHandle(p r0)'
      Goto rela_ready_${RelaCheckId}
    ${EndIf}
    ; The updater launches NSIS just before exiting the old GUI. Give that
    ; exact directory a bounded interval to become replaceable.
    ${If} $UpdateMode = 1
    ${AndIf} $1 < 60
      IntOp $1 $1 + 1
      Sleep 500
      Goto rela_wait_${RelaCheckId}
    ${EndIf}
    Pop $1
    Pop $0
    SetErrorLevel 2
    Abort "请退出此安装目录中的 ${productName} 后重试。程序文件仍被占用或没有替换权限；其他副本未被关闭。"
  rela_ready_${RelaCheckId}:
    Pop $1
    Pop $0
  !undef RelaCheckId
!macroend

; ProgramData's service is shared with Portable copies and other users. Software
; maintenance must not infer that uninstalling this GUI authorizes removing it.
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro RelaRefusePending
  DetailPrint "保留共享的 Rela 网络服务和网络配置。彻底清理服务请使用 Remove-Network-Service.cmd 并确认。"
!macroend
