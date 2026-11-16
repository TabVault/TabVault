; TabVault NSIS hooks.
;
; Runs inside Tauri's generated NSIS installer for TabVault. The installer
; copies the app, `tabvault-host.exe` and `register-native-host.ps1` into
; $INSTDIR (current user's Programs folder) and then runs these hooks.
;
; POSTINSTALL registers the native messaging host with Chrome, Edge and
; Firefox (registry + manifest). If that fails we warn but do not fail the
; install — the app itself is fully functional and the user can re-run the
; registration manually at any time.
;
; PREUNINSTALL removes the registry keys so a reinstall starts clean.

!macro NSIS_HOOK_POSTINSTALL
  ; Register the native host. The script is idempotent and works unattended.
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -NonInteractive -File "$INSTDIR\register-native-host.ps1" -AppDir "$INSTDIR"' $R0
  IntCmp $R0 0 registration_ok registration_failed registration_failed

  registration_failed:
    MessageBox MB_ICONEXCLAMATION "TabVault was installed, but automatic native-host registration reported an error (code $R0). The app itself works fine; to enable browser capture, run once as your user: powershell -ExecutionPolicy Bypass -File register-native-host.ps1 -AppDir $INSTDIR"

  registration_ok:
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Remove the browser registry entries. The manifest stays behind; a reinstall
  ; rewrites it. Nothing user-generated is touched.
  ExecWait '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -NonInteractive -File "$INSTDIR\register-native-host.ps1" -AppDir "$INSTDIR" -Uninstall' $R0
!macroend
