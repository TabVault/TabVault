# Extension permissions, and why each one exists

TabVault's extension asks for these permissions. Every one of them is required
for the core promise — "you can never lose a tab again" — and none of them is
used for anything else. There is **no** network access, and there never will
be; TabVault stores everything locally.

## `tabs`

The whole point. Without it the extension cannot see URLs, titles, pinned
state, tab indices, separators between tabs, which window a tab lives in,
whether a window is focused, or whether a tab was just activated. All of that
is recorded so a restore can reproduce the session faithfully — including
**pinned tabs reopening pinned**.

## `tabGroups`

Tab groups are recorded with their name and color so a restored window comes
back *organized*, not as a flat pile of 200 tabs. Split across two APIs:
`tabs.group` creates the group and `tabGroups.update` names/colors it — both
are needed and both are used only during restore. In Chrome and Edge this is a
required permission; in Firefox it ships as `optional_permissions`, so the
extension installs there even on builds without group support (groups are
silently skipped — everything else works).

## `storage`

`storage.session` holds the per-tab runtime counters the extension must
maintain itself: creation times, per-tab visit counts, accumulated active time,
and the recent-close list used for mass-close detection. `storage.local` holds
the **offline queue**: if the native host is down (browser restart, host not
installed yet), every event is queued there and flushed when the host returns.
This is the "nothing is ever lost" guarantee.

## `nativeMessaging`

The bridge to the local vault. The extension talks to `tabvault-host.exe`
using Chrome's native messaging (length-prefixed JSON over stdio). No ports
are opened, nothing listens on the network. Chrome and Edge validate the
extension's origin against `allowed_origins` in the host manifest; Firefox
validates the add-on ID against `allowed_extensions`.

## `alarms`

The 60-second (configurable) capture timer. MV3 kills idle service workers;
`chrome.alarms` is the supported way to wake reliably. It also drives the
30 s "poll for restore jobs" timer and the once-a-minute queue flush.

## `notifications`

The mass-close toast: *"22 tabs just closed. Restore?"* Clicking it tells the
extension to launch the desktop app via the native host. No other
notifications are ever sent.

## `history`

Used **only** by the optional one-shot "Seed visit statistics from history"
button in the extension's Options page. It seeds Most Used with rough visit
counts from the last 90 days. It is never used in the background, never
uploaded, never exported. The desktop app can read the browser's `History`
SQLite directly (read-only, after copying) which is the stronger path — this
button exists for users who prefer not to grant file access.

## `sessions`

Used **only** by the optional one-shot "Import recently closed tabs" button in
the extension's Options page. It reads `browser.sessions.getRecentlyClosed()`
(up to 25 entries) once, on explicit user action, and writes them into the
vault. It is never read passively, on a timer, or at startup.

### `commands` (`Alt+Shift+V`)

Not a permission, but listed for completeness: opens the TabVault popup. You
can rebind it in `chrome://extensions/shortcuts`.

---

## Why not `<other permission>`?

* **No `webRequest` / `declarativeNetRequest`** — TabVault does not inspect,
  modify, block or read the *content* of any page.
* **No `cookies`, no `bookmarks`** — not needed; bookmarks are out of scope.
* **No `<all_urls>` / host permissions** — TabVault never reads page content,
  forms, passwords or cookies. It records tab *metadata* only.
* **No `identity`, `sync`, `management`** — there is no TabVault account and
  no cloud to sync to.
* **No background page persistence tricks** — MV3 service worker + alarms is
  the supported, battery-friendlier model.

## What the host sees

The native host receives: URLs, titles, favicon URLs, pins, indices, group
names/colors, window ids, incognito flags (filtered out unless you opt in),
per-tab active time, per-tab visit counts, and your extension's version. That
is the complete list — see `crates/tv-host/src/protocol.rs`; it is short on
purpose.

## What gets stored, and for how long

Everything lands in `%APPDATA%\TabVault\tabvault.db` (SQLite, WAL). Snapshots
age out per the retention tiers in Settings (default: keep everything 24 h,
hourly for 30 days, daily for 1 year). **Closed-tab and closed-window history
is permanent** — that is the product. `backups/` keeps 14 days of daily copies
(configurable). History/visit stats age out only when they carry no signal.

## Uninstalling cleanly

* Remove the extension (`chrome://extensions` → Remove).
* Run `installer\scripts\register-native-host.ps1 -Uninstall` (or uninstall
  from Settings → Apps, which runs the same step).
* Delete `%APPDATA%\TabVault` if you want your recorded history gone too.
