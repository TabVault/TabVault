; TabVault NSIS hooks.
;
; Runs inside Tauri's generated NSIS installer for TabVault.
;
; POSTINSTALL — registers the native messaging host for Chrome, Edge and Firefox.
;   Everything here is native NSIS: no scripts are spawned, nothing is downloaded.
;   The host manifest is written next to tabvault-host.exe with the install path
;   interpolated. Registration is per-user (HKCU), exactly what a browser
;   extension needs to talk to the host; no elevated access is used.
;
; PREUNINSTALL — removes those HKCU registry keys so a reinstall starts clean.

!macro NSIS_HOOK_POSTINSTALL
  ; The manifest lists absolute allowed origins, the browser extension ids and
  ; the host binary path. Forward slashes need no JSON escaping and are handled
  ; by Windows APIs transparently.
  FileOpen $9 "$INSTDIR\com.tabvault.host.json" w
  FileWrite $9 '{"name":"com.tabvault.host","description":"TabVault native messaging host: browser extension to local SQLite vault","path":"$INSTDIR/tabvault-host.exe","type":"stdio","allowed_origins":["chrome-extension://fbeikljjdhlgipmpgeohjigbgdmnkeae/"],"allowed_extensions":["tabvault@tabvault"]}'
  FileClose $9

  ; Chrome and Edge validate the extension's origin against allowed_origins;
  ; Firefox validates the add-on id against allowed_extensions.
  WriteRegStr HKCU "Software\Google\Chrome\NativeMessagingHosts\com.tabvault.host" "" "$INSTDIR\com.tabvault.host.json"
  WriteRegStr HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.tabvault.host" "" "$INSTDIR\com.tabvault.host.json"
  WriteRegStr HKCU "Software\Mozilla\NativeMessagingHosts\com.tabvault.host" "" "$INSTDIR\com.tabvault.host.json"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Remove only the keys this installer wrote. Nothing user-generated is touched.
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.tabvault.host"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.tabvault.host"
  DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.tabvault.host"
!macroend
