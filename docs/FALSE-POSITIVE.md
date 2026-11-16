# About Antivirus "False Positive" Reports

This document explains why a TabVault installer might be flagged by a few
antivirus vendors, what has been done to prevent it, and how to report a false
positive to the vendors if you see one.

## What a false positive is

Antivirus engines use heuristics and machine learning to catch *unknown*
threats. They have never seen this exact binary before, it is unsigned, and
Windows executables that compress their payloads (as most installers do) look
suspicious to a statistical model even when they contain nothing malicious.
A detection by one or two engines on a freshly built, unsigned installer is
almost always a **false positive**.

## What TabVault does to minimize false positives

Everything below is verifiable in the source:

1. **No network access at install time.** The installer does **not** download
   anything. (Most Tauri bundles download a WebView2 bootstrapper at install
   time — TabVault explicitly skips that and uses the WebView2 runtime already
   present on Windows 10/11.)
2. **No scripts are executed.** The installer contains no PowerShell, no
   `cmd.exe`, no batch, no embedded script of any kind. Native host
   registration is done with plain registry writes inside the installer.
3. **The app spawns no child processes** for normal operation (registry writes
   use the Win32 API directly; URLs are opened with `ShellExecuteW`).
4. **Full identity metadata** is baked into the installer and the binaries:
   product name, publisher, version, MIT license text and copyright string —
   see `app/src-tauri/tauri.conf.json` (`bundle.publisher`, `copyright`,
   `licenseFile`, `homepage`).
5. **No packed/obfuscated stubs beyond what every NSIS installer uses** — the
   installer is built with Tauri's standard NSIS pipeline from auditable
   source. There is no UPX or custom packer.
6. The app makes **no network connections** at runtime. It is fully offline.
   Static analysis tools that attempt network calls find none to find.
7. **Windows Defender** (Microsoft's engine, which backs Windows Security and
   is represented in essentially every multi-scanner verdict set) scans the
   current build and reports **no threats**.

## Current hashes

| File | SHA-256 |
|------|---------|
| `TabVault_1.0.0_x64-setup.exe` | `387584946357B13601C8D93E7B6E6DF29EA878722EDF63A0C4081EB7D9FF8015` |
| `tabvault.exe` (app) | `4F6EF5F374FBE1713EC772BBDDA7033E5E85990FFA8C16E73423C4A45E4D83B0` |
| `tabvault-host.exe` (native host) | `FF4AB3B39526F32ED59141BF709A800EF5F89E8BFC78E416847DD7147B8E033D` |

These change with every build. Always verify against the release page.

## If your antivirus flags it anyway

This is an unsigned binary from a small independent project with no install
base yet, so it has no reputation. Two steps fix this permanently:

1. **Report the false positive to the flagging vendor.** Every major vendor
   has a submission form; most clear it within days:
   - Microsoft: upload the file at the Microsoft Security Intelligence portal
     ("Submit a file for malware analysis") — type: false positive.
   - Arctic Wolf, SecureAge, Bkav: each has an "undetected threat" /
     "false positive" report form on their site (or reply to their support
     mail) with the hash from the table above.
2. **Reputation builds over time** automatically as more copies are installed;
   or an **Authenticode certificate** (OV or EV) can be purchased to sign
   releases — signing is the single most effective long-term fix for
   unsigned-software warnings. TabVault does not yet ship signed builds
   because certificates are a recurring cost paid by the project authors.

## What TabVault will never do

- Download or execute additional code after install.
- Hide its actions. Every installer action is visible in
  `app/src-tauri/nsis-hook.nsh` and the generated
  `target/release/nsis/x64/installer.nsi`.
- Renew/rotate itself silently.
- Phone home, collect telemetry, or bundle adware/offers.

If a build of TabVault behaves differently from this document, **it is not a
TabVault release** — do not install it and report it to the project issue
tracker.

## Where the registration script fits in

`installer/scripts/register-native-host.ps1` is used only for **source builds
and development installs** (it is documented in the README for loading the
host from a custom directory). The NSIS installer does not bundle or execute
it — it registers the host inline in `app/src-tauri/nsis-hook.nsh` with plain
registry writes, so what gets installed is exactly what you can read in those
two files.
