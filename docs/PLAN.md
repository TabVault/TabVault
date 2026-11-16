# TabVault — Plan

## Status (M0–M6 build complete and verified end-to-end)

- [x] **M1** — SQLite vault (WAL, SQLCipher-ready, 3 migrations), native
      messaging host, snapshot diffing/fingerprinting, retention, SNSS
      (Session_*/Tabs_*) + mozlz4 (Firefox) parsers. 73+ unit tests green.
- [x] **M2** — Extension capture (MV3, Chrome/Edge/Firefox, polyfill),
      offline queue with flush-on-reconnect, native-host registration script
      (HKCU, all three browsers), verified live in Chrome for Testing.
- [x] **M3** — Tauri tray app: Recently Closed, Timeline, Pinned Vault, Most
      Used, Ctrl+K global search, Ctrl+Alt+R restore-hotkey, tray menu,
      virtualized lists.
- [x] **M4** — Recovery from the past: auto-detects Chrome/Edge/Firefox
      profiles, parses SNSS (v3 cleartext verified against real files; v5
      encrypted v20 App-Bound detected and reported, not silently empty),
      mozlz4 sessionstore, history DBs — locked-file copy protocol.
- [x] **M5** — Installer: `TabVault_1.0.0_x64-setup.exe` (NSIS, per-user
      install) with native-host registration hook; portable registration
      script.
- [x] **M6** — Encryption at rest (DPAPI-wrapped SQLCipher key), daily backup,
      autostart, retention, exports (JSON/HTML), import seed, settings UI.

### Verified end-to-end on this machine

- Live capture → vault → restore job → extension reopens window with pins.
- Offline queue: events queued while host is down; all flush on reconnect.
- Mass-close detection suppressed at browser startup (window-migration fix);
  true mass closes still flag recovery points.
- Tab identity by browser tab id — no phantom closes on tab reorder.
- Recovered real session files on this machine: 72 tabs / 24 windows from
  Chrome's `Tabs_*`, closed windows grouped with close timestamps.
- Full docs: README, PERMISSIONS, MANUAL-TEST-PLAN.

## Stack decision: Tauri 2 + React 19 + TypeScript (not Electron)

| Criterion | Tauri 2 | Electron |
|---|---|---|
| Idle RAM (our NFR) | ~35–60 MB (WebView2 is already resident in Windows) | ~120–190 MB (bundles its own Chromium) |
| Installer size | ~6–10 MB | ~90–150 MB |
| Idle CPU | negligible native event loop | separate renderer + GPU process |
| Native messaging host | same process, shared Rust crate, no IPC hop | needs a **separate** Node host binary |
| DB + parsers | one Rust crate, no FFI marshalling | Node native addons for SQLCipher |
| Tray/autostart/DPAPI/notifications | first-class Tauri plugins | heavier, third-party |

Electron would be the right call if we needed a *specific* Chromium version or heavy Chrome-DevTools-Protocol work. We need none of that, and we have a hard **<100 MB RAM / <1 % CPU** requirement — Tauri wins on every axis that matters here.

The **native messaging host is the same Rust library** (`tv-core`) compiled into a second tiny binary (`tabvault-host`). One schema, one parser set, one set of tests. The extension talks to the host; the host writes to the same SQLite file the UI reads. No HTTP, no ports.

## Folder structure

```
TabVault/
├─ crates/
│  ├─ tv-core/            # library: schema, migrations, snapshot diff, retention,
│  │                      #          parsers (SNSS, mozlz4, places.sqlite), DPAPI, backup
│  └─ tv-host/            # native-messaging host binary (stdin/stdout framing)
├─ app/                   # Tauri 2 desktop app
│  ├─ src-tauri/          # Rust shell: tray, commands, autostart, toast, global hotkey
│  └─ src/                # React + TS UI (virtualized)
├─ extension/             # one MV3 codebase → chrome / edge / firefox builds
├─ installer/             # NSIS + WiX(MSI) + PowerShell native-host registration
├─ tests/fixtures/        # real-format binary fixtures + expected parses
└─ docs/                  # README, architecture, permissions, test plan
```

## Milestones (build order, with a runnable checkpoint each)

| # | Deliverable | Test checkpoint |
|---|---|---|
| **M1** | SQLite schema + migrations + WAL, snapshot diff, retention, native host + pipe framing, PowerShell registry registration | `cargo test -p tv-core`; `tabvault-host --selftest`; pipe round-trip |
| **M2** | Extension capture (MV3, polyfill, chrome/edge/firefox manifests, offline queue + flush) | Load unpacked; close tabs; rows appear in DB |
| **M3** | Tray app, Recently Closed (virtualized), restore engine | Tray menu; restore single/window/session |
| **M4** | Timeline, Pinned Vault, Most Used, global search, hotkey | Browse snapshots; re-pin all |
| **M5** | Session-file recovery: Chrome SNSS (`Session_*`/`Tabs_*`), Firefox `sessionstore-backups` (mozlz4) + `places.sqlite` | Recover tabs that predate the extension |
| **M6** | NSIS/MSI installer, native-host registration step, autostart, backup, DPAPI encryption, docs, full test suite | End-to-end install on clean profile |

## Key verified facts (from primary sources, not guessed)

- **Chrome session files are SNSS.** Header `SSNS` (0x53534E53) + `int32 version`. Cleartext version is **3** (has an initial-state marker command id **255**); version **5** is OSCrypt-encrypted (we detect and report, cannot decrypt without Chrome's key). Record framing = `uint16 size` + `uint8 id` + `size-1` payload bytes. (Version 1 is dead; versions 2/4 never shipped.)
- Session command IDs (authoritative, `session_service_commands.cc`): 0 SetTabWindow, 2 SetTabIndexInWindow, 6 UpdateTabNavigation, 7 SetSelectedNavIndex, 8 SetSelectedTabInIndex, 9 SetWindowType, 12 SetPinnedState, 14 SetWindowBounds3, 16 TabClosed, 17 WindowClosed, 19 SessionStorageAssociated, 20 SetActiveWindow, 21 LastActiveTime, 25 SetTabGroup, 27 SetTabGroupMetadata2, 28 SetTabGuid, 31 SetWindowUserTitle, 34 AddWindowExtraData, **255 InitialStateMarker**.
- `Tabs_*` files use a **different** ID space: 1 UpdateTabNavigation, 2 RestoredEntry, 4 SelectedNavigationInTab, 5 PinnedState, 6 SetExtensionAppID, 9 Window, 10 SetTabGroupData, 13 CreateGroup, 14 AddTabExtraData.
- Pickle: `uint32 payload_len` header, 4-byte aligned, string = len+bytes+pad, string16 = char-count+UTF16LE+pad, bool = 4-byte int. All ints 4 bytes (I read exactly 4, not the source's `sizeof(SessionID::id_type)`).
- **Firefox** sessionstore = `mozLz40\0` + `uint32 LE uncompressed size` + **raw LZ4 block** (not frame). Timestamps are ms since Unix epoch.
- Native messaging: `uint32` length prefix, native byte order, **1 MiB host→browser cap**; Chrome registry `HKCU\Software\Google\Chrome\NativeMessagingHosts\<name>`, Edge `...\Microsoft\Edge\...`, Firefox `HKCU\Software\Mozilla\NativeMessagingHosts\<name>`.

## Assumptions (stated, not asked)

1. Windows 10/11 x64, per-user install, no admin required.
2. One extension ID each for Chrome/Edge (pinned via `key` in manifest) and one Firefox add-on ID so `allowed_origins`/`allowed_extensions` are stable.
3. "Active time" = time the tab was the active tab in its focused window, measured by the extension via `tabs.onActivated` + `windows.onFocusChanged`.
4. Skip-incognito defaults on; domain exclusion list is suffix-matched.
5. Snapshots are deduped by a stable content hash, so a 60 s timer on an idle browser writes nothing.
