# TabVault â€” Manual Test Plan

This is the human half of the verification story. The automated half is
`cargo test --workspace` (70+ tests covering schema/migrations, retention,
snapshot diffing, SNSS/mozlz4 parsers, DPAPI, SQLCipher round-trips and WAL).

Run these **in order**. Each step builds on the previous one. Estimated
time: 30â€“45 minutes.

## Preconditions

1. TabVault installed (or run from source, `cargo run`), extensions loaded as
   described in the README for your browser.
2. `%APPDATA%\TabVault\tabvault.db` exists.
3. A test browser profile you are comfortable closing windows in. (Tabs you
   restore in these steps can be closed again afterwards.)
4. The extension popup shows **"host connected"**.

---

## 1. Basic capture

| # | Step | Expected result |
|---|------|-----------------|
| 1.1 | Open 5 tabs (github.com, news.ycombinator.com, example.com, bing.com, wikipedia.org). | Nothing visible yet â€” capture is silent by design. |
| 1.2 | Popup â†’ **Snapshot now**. Open the app â†’ **Recently Closed** (or Timeline). | A snapshot with exactly those 5 tabs in that order is recorded; window focused flag is visible in the detail pane. |
| 1.3 | Close one tab. | Within ~2 seconds, *Recently Closed* lists it (title, URL, domain, relative time, pinned state). |
| 1.4 | Pin two of the tabs. Reload the popup. | Pinned badge appears on those tabs in the timeline detail; the pinned state survives snapshots. |
| 1.5 | Close the whole window with 4 tabs still open. | All 4 appear in Recently Closed tagged with the same window marker; the **window** group is visually grouped. |

## 2. Mass-close recovery point (the headline feature)

| # | Step | Expected result |
|---|------|-----------------|
| 2.1 | Open 8 tabs. Pin 3 of them. | â€” |
| 2.2 | Close the entire window (X button). | Within ~5 seconds: the vault records a **Recovery Point**; a Windows toast appears: *"N tabs just closed. Restore?"* (If toasts are disabled in Settings â†’ Notifications, verify the recovery point exists in the Timeline instead.) |
| 2.3 | In the app â†’ **Recently Closed** â†’ filter **Pinned only**. | The 3 pinned tabs from the closed window are listed. |
| 2.4 | Select them â†’ **Restore 3 in new window**. | A new browser window opens with the 3 tabs, **pinned**. |
| 2.5 | Filter to the **source** "Live capture". | The recovery rows carry it; the rest are unaffected. |

## 3. Restore semantics

| # | Step | Expected result |
|---|------|-----------------|
| 3.1 | Recently Closed â†’ check **skip already-open** â†’ restore a tab you already have open elsewhere. | Nothing opens (correctly skipped). |
| 3.2 | Same, with the checkbox off. | The tab opens again (duplicate, deliberately). |
| 3.3 | Restore a full window from Recently Closed via **Restore window**. | All its tabs reopen, pinned ones pinned, in one new window. |
| 3.4 | Re-pin all: Pinned Vault â†’ **Re-pin all**. | Every previously pinned site opens pinned in one new window; already-open ones are skipped (checkbox). |

## 4. Timeline fidelity

| # | Step | Expected result |
|---|------|-----------------|
| 4.1 | Timeline â†’ pick any snapshot. | Windows, tab order, pins, groups shown exactly as captured. |
| 4.2 | **Restore this snapshot**. | The whole layout reopens in a new window. |
| 4.3 | With exclusions configured (Settings â†’ Recording â†’ Exclusions: `example.com`), open example.com tabs, snapshot, and check the snapshot. | example.com tabs are **absent** from the snapshot and from Recently Closed; the rest are recorded. |

## 5. Pause + retention

| # | Step | Expected result |
|---|------|-----------------|
| 5.1 | Tray or Settings â†’ **Pause recording**. Open/close tabs. | No new snapshots, no Recently Closed rows. Unpause â†’ capture resumes. |
| 5.2 | Settings â†’ retention hourly = 7 days, daily = 90, press **Prune now**. | A dry-run summary shows kept recent/hourly/daily and deleted counts; closed-tab history is untouched (it is permanent). |

## 6. Recover from the past (the anti-disaster feature)

| # | Step | Expected result |
|---|------|-----------------|
| 6.1 | Before doing anything, in your browser close a window with 3 tabs you care about. Wait â‰¤ 1 minute (session files are rewritten on an interval). | â€” |
| 6.2 | TabVault â†’ **Recover from the past**. | Your browser profiles are listed with every session/history file found, sizes and mtimes. |
| 6.3 | Click **Import** on the newest `Tabs_*` file. | Import reports "N tabs imported". They appear in Recently Closed with the `Chrome/Edge file` source badge. |
| 6.4 | Import the Firefox `recovery.jsonlz4` (if present). | Same flow; badge `Firefox file`. |
| 6.5 | Import the history `History` / `places.sqlite`. | Visit counts appear in **Most Used**; no tab rows are created from history. |
| 6.6 | Import the same file twice. | Second import creates no duplicates (dedupe by URL key + close time window). |
| 6.7 | Import the `Sessions_Encrypted` (v20) file. | Clear explanation: *App-Bound Encryption â€” unreadable outside the browser; import the cleartext `Sessions/` mirror instead.* Nothing is fabricated. |

## 7. Encryption at rest (optional)

| # | Step | Expected result |
|---|------|-----------------|
| 7.1 | Settings â†’ **Encrypt database**. | Toggle succeeds; vault recreated encrypted; backups folder contains `tabvault-pre-migration-*.db`. |
| 7.2 | Restart the app with `TABVAULT_DB=â€¦` unset. | App opens normally (key unwraps transparently via DPAPI). |
| 7.3 | Try to open `%APPDATA%\TabVault\tabvault.db` in any SQLite tool. | "file is not a database" â€” no schema, no rows. |
| 7.4 | Toggle **off**. | Vault returns to plaintext; all data intact (verified by counts before/after). |

## 8. Tray + shortcuts

| # | Step | Expected result |
|---|------|-----------------|
| 8.1 | Close the window with X. | App remains in the tray (if "minimize to tray" is on). |
| 8.2 | Tray â†’ **Restore Last Closed Window** (or `Ctrl+Alt+R`). | The most recent closed window's tabs reopen. |
| 8.3 | `Ctrl+K` â†’ type a distinctive word from an old tab. | Result list shows it; Enter opens it in the browser. |
| 8.4 | `Alt+1`â€¦`Alt+6`. | Jumps to each screen. |

## 9. Restore-feedback & keyboard

| # | Step | Expected result |
|---|------|-----------------|
| 9.1 | Recently Closed â†’ select rows with the keyboard: press `Tab` to the list, `â†“`/`â†‘` to move, `Space` to toggle selection, `Esc` to clear. | Focus highlight follows the arrow keys; selection toggles; Esc clears. |
| 9.2 | Timeline â†’ focus the snapshot list, use `â†“`/`â†‘` (or `Home`/`End`) to move the selection, `Enter` to restore. | The detail pane follows the selection; Enter restores that snapshot into a new window. |
| 9.3 | Trigger a restore with the browser closed. | The toast reports the dispatch outcome (sent / failed / "tabs will open on next launch"), not just "restoringâ€¦". |
| 9.4 | Import a `Tabs_*` file, then **Recently Closed â†’ Group by source file**. | A banner per imported file (name, import time, counts) with that file's tabs under it; live captures sit under a "Live capture" banner. |

## 10. Failure drills

| # | Step | Expected result |
|---|------|-----------------|
| 9.1 | Restart the browser (so the service worker restarts). | Popup re-shows "host connected"; queued events flush (visible if you had any pending). |
| 9.2 | Rename `com.tabvault.host.json`'s target in the registry to a bogus path. | Popup says host offline with the exact reason; app still opens; history untouched. Restore the key to fix. |
| 9.3 | Start two browsers at once (Chrome + Edge). | Both capture independently into the same vault (different `browser` values). |
