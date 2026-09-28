; Agent Notch's additions to Tauri's NSIS installer (DESIGN-WIN §6.5), included by the template
; through bundle.windows.nsis.installerHooks. The deep-link scheme is registered by the template
; itself (plugins.deep-link in tauri.conf.json), the autostart Run value is deleted by it, and
; %LOCALAPPDATA%\com.rivantmedia.agentnotch (which holds <support>) goes with its "Delete the
; application data" checkbox.

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ; The running app is asked to quit, so it stops its hub (stores saved, held PermissionRequests
    ; closed without an answer: each waiting hook exits 0 and Claude Code's own prompt decides)
    ; before its files are removed. Its exit code doesn't matter: no instance is fine too.
    Push $0
    nsExec::Exec '"$INSTDIR\agentnotch.exe" control quit'
    Pop $0
    Sleep 1500
    ; Hooks are removed only on an explicit request: "Delete the application data" (set by the
    ; confirm page before this section runs) or /REMOVEHOOKS. The "Uninstall before installing"
    ; step of a manual upgrade runs this uninstaller without /UPDATE and must leave every
    ; settings.json alone; entries left behind exit 0 at once while the app is gone.
    Push $R9
    ClearErrors
    ${GetOptions} $CMDLINE "/REMOVEHOOKS" $R9
    ${If} $DeleteAppDataCheckboxState = 1
    ${OrIfNot} ${Errors}
      nsExec::Exec '"$INSTDIR\agentnotch.exe" uninstall-hooks --quiet'
      Pop $0
    ${EndIf}
    Pop $R9
    Pop $0
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; Upstream's config and logs live under productName, which the template doesn't know about;
    ; a sealed run keeps its own copy beside them.
    SetShellVarContext current
    RMDir /r "$APPDATA\Agent Notch"
    RMDir /r "$APPDATA\Agent Notch Sealed"
  ${EndIf}
!macroend
