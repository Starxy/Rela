; Uninstall only Rela's service. Other EasyTier installations are unaffected.
!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog '"$SYSDIR\sc.exe" stop RelaEasyTier'
  Pop $0
  nsExec::ExecToLog '"$SYSDIR\sc.exe" delete RelaEasyTier'
  Pop $0
!macroend
