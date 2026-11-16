//! TabVault desktop app: the tray host, the WebView UI, and every window the
//! user sees. All heavy work (parsing, retention, restore enqueues) happens in
//! `tv-core`; this module only wires it to the UI.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::Manager;
use tv_core::db::{open_vault, query, write, Db};
use tv_core::model::{BrowserSnapshot, Settings, TabRecord};

pub struct AppState {
    /// `None` only while an encryption migration is in flight; every command
    /// surfaces a clear "busy" error in that brief window instead of crashing.
    pub db: Mutex<Option<Db>>,
}

fn vault_path() -> Result<PathBuf, String> {
    if let Some(override_path) = std::env::var_os("TABVAULT_DB") {
        return Ok(PathBuf::from(override_path));
    }
    let appdata = std::env::var_os("APPDATA").ok_or_else(|| "APPDATA is not set".to_string())?;
    Ok(PathBuf::from(appdata).join("TabVault").join("tabvault.db"))
}

fn db<'a>(state: &'a tauri::State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Option<Db>>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
}

/// Borrow the vault, or fail fast if an encryption migration is in flight.
fn vault<'a>(guard: &'a std::sync::MutexGuard<'_, Option<Db>>) -> Result<&'a Db, String> {
    guard
        .as_ref()
        .ok_or_else(|| "vault is briefly unavailable while an encryption migration runs".to_string())
}

// ---------------------------------------------------------------------------
// Restore payload helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreMode {
    pub new_window: bool,
    pub skip_open: bool,
}

impl Default for RestoreMode {
    fn default() -> Self {
        Self {
            new_window: true,
            skip_open: false,
        }
    }
}

fn restore_payload_for_tabs(
    tabs: &[TabRecord],
    mode: RestoreMode,
) -> serde_json::Value {
    let entries: Vec<serde_json::Value> = tabs
        .iter()
        .enumerate()
        .map(|(i, t)| {
            serde_json::json!({
                "url": t.url,
                "pinned": t.pinned,
                "active": i == 0,
                "groupName": t.group_name,
                "groupColor": t.group_color,
            })
        })
        .collect();
    serde_json::json!({
        "tabs": entries,
        "newWindow": mode.new_window,
        "skipOpen": mode.skip_open,
    })
}

fn tab_from_closed_row(row: &query::ClosedTabRow) -> TabRecord {
    TabRecord {
        url: row.url.clone(),
        title: row.title.clone(),
        favicon: row.favicon.clone(),
        pinned: row.pinned,
        tab_index: 0,
        window_id: row.window_event_id.map(|id| id.to_string()).unwrap_or_default(),
        group_name: row.group_name.clone(),
        group_color: row.group_color.clone(),
        browser: row.browser.clone(),
        profile: row.profile.clone(),
        opened_ms: row.opened_ms,
        closed_ms: Some(row.closed_ms),
        last_active_ms: row.last_active_ms,
        active_ms: row.active_ms,
        visit_count: row.visit_count,
        part_of_closed_window: row.part_of_closed_window,
        incognito: false,
        source: tv_core::model::Source::Extension,
        confidence: tv_core::model::Confidence::High,
        browser_tab_id: None,
    }
}

/// Enqueues a restore job; returns the job id (also shown in the UI).
pub(crate) fn enqueue_restore(
    db: &Db,
    action: &str,
    payload: serde_json::Value,
) -> Result<i64, String> {
    write::enqueue_restore_request(db, action, &payload).map_err(|e| e.to_string())
}

/// Shared implementation for the tray item, the global hotkey and the UI
/// command: restore the most recent closed window (or do nothing when there
/// is none).
pub(crate) fn restore_last_closed_window_impl(db: &Db) -> Result<Option<i64>, String> {
    let conn = db.conn();
    let recent = query::list_closed_windows(conn, None, 1, 0).map_err(|e| e.to_string())?;
    let Some(window) = recent.into_iter().next() else {
        return Ok(None);
    };
    let tabs = query::closed_window_tabs(conn, window.id).map_err(|e| e.to_string())?;
    if tabs.is_empty() {
        return Ok(None);
    }
    let tab_records: Vec<TabRecord> = tabs.iter().map(tab_from_closed_row).collect();
    let payload = restore_payload_for_tabs(&tab_records, RestoreMode::default());
    enqueue_restore(db, "open_window", payload).map(Some)
}

// ---------------------------------------------------------------------------
// Tauri commands: dashboard and recently closed
// ---------------------------------------------------------------------------

#[tauri::command]
fn dashboard(state: tauri::State<'_, AppState>) -> Result<query::DashboardStats, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::dashboard_stats(db.conn()).map_err(|e| e.to_string())
}

#[tauri::command]
fn browser_profiles(state: tauri::State<'_, AppState>) -> Result<Vec<query::BrowserProfileRow>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::browser_profiles(db.conn()).map_err(|e| e.to_string())
}

#[tauri::command]
fn recently_closed(
    state: tauri::State<'_, AppState>,
    filter: query::ClosedTabFilter,
) -> Result<query::ClosedTabPage, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::list_closed_tabs(db.conn(), &filter).map_err(|e| e.to_string())
}

#[tauri::command]
fn import_batches(state: tauri::State<'_, AppState>) -> Result<Vec<query::ImportBatchRow>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::list_import_batches(db.conn()).map_err(|e| e.to_string())
}

#[tauri::command]
fn closed_windows(
    state: tauri::State<'_, AppState>,
    browser: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<query::ClosedWindowRow>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::list_closed_windows(
        db.conn(),
        browser.as_deref(),
        limit.unwrap_or(100),
        offset.unwrap_or(0),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn closed_window_tabs(
    state: tauri::State<'_, AppState>,
    window_event_id: i64,
) -> Result<Vec<query::ClosedTabRow>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::closed_window_tabs(db.conn(), window_event_id).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Restore commands
// ---------------------------------------------------------------------------

#[tauri::command]
fn restore_tabs(
    state: tauri::State<'_, AppState>,
    ids: Vec<i64>,
    mode: RestoreMode,
) -> Result<i64, String> {
    if ids.is_empty() {
        return Err("no tabs selected".to_string());
    }
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let conn = db.conn();
    // One batched query (no per-id round trips), ordered so the restored
    // window keeps the tabs in visual order: by close time (oldest first)
    // then by original tab index.
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT id, url, title, favicon, pinned, browser, profile, opened_ms,
                closed_ms, last_active_ms, active_ms, visit_count, group_name,
                group_color, window_event_id, source_batch_id
         FROM closed_tab_events
         WHERE id IN ({placeholders})
         ORDER BY closed_ms ASC, tab_index ASC, id ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let tabs = stmt
        .query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            let url: String = r.get(1)?;
            Ok(query::ClosedTabRow {
                id: r.get(0)?,
                title: r.get(2)?,
                favicon: r.get(3)?,
                domain: tv_core::url_util::registrable_domain(&url),
                url,
                pinned: r.get::<_, i64>(4)? != 0,
                browser: r.get(5)?,
                profile: r.get(6)?,
                opened_ms: r.get(7)?,
                closed_ms: r.get(8)?,
                last_active_ms: r.get(9)?,
                active_ms: r.get(10)?,
                visit_count: r.get(11)?,
                group_name: r.get(12)?,
                group_color: r.get(13)?,
                part_of_closed_window: false,
                source: "extension".to_string(),
                confidence: "high".to_string(),
                window_event_id: r.get(14)?,
                source_batch_id: r.get(15)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;
    if tabs.is_empty() {
        return Err("none of the selected tabs could be found in the vault".to_string());
    }
    let tab_records: Vec<TabRecord> = tabs.iter().map(tab_from_closed_row).collect();
    let payload = restore_payload_for_tabs(&tab_records, mode);
    enqueue_restore(&db, "open_window", payload)
}

#[tauri::command]
fn restore_window(
    state: tauri::State<'_, AppState>,
    window_event_id: i64,
    mode: RestoreMode,
) -> Result<i64, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let tabs = query::closed_window_tabs(db.conn(), window_event_id).map_err(|e| e.to_string())?;
    if tabs.is_empty() {
        return Err("that window has no recorded tabs".to_string());
    }
    let tab_records: Vec<TabRecord> = tabs.iter().map(tab_from_closed_row).collect();
    let payload = restore_payload_for_tabs(&tab_records, mode);
    enqueue_restore(&db, "open_window", payload)
}

#[tauri::command]
fn restore_snapshot(
    state: tauri::State<'_, AppState>,
    snapshot_id: i64,
    mode: RestoreMode,
) -> Result<i64, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let snap = query::snapshot_detail(db.conn(), snapshot_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "snapshot not found".to_string())?;
    let tabs: Vec<TabRecord> = snap
        .windows
        .iter()
        .flat_map(|w| w.tabs.iter().cloned())
        .collect();
    if tabs.is_empty() {
        return Err("that snapshot captured no tabs".to_string());
    }
    let payload = restore_payload_for_tabs(&tabs, mode);
    enqueue_restore(&db, "open_window", payload)
}

#[tauri::command]
fn restore_last_closed_window(state: tauri::State<'_, AppState>) -> Result<Option<i64>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    restore_last_closed_window_impl(&db)
}

#[tauri::command]
fn recent_restore_requests(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<(i64, String, String, String, Option<i64>)>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    write::recent_restore_requests(&db, 50).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------

#[tauri::command]
fn timeline(
    state: tauri::State<'_, AppState>,
    from: Option<i64>,
    to: Option<i64>,
    browser: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<query::SnapshotRow>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::list_snapshots(
        db.conn(),
        from,
        to,
        browser.as_deref(),
        limit.unwrap_or(200),
        offset.unwrap_or(0),
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn snapshot_detail(
    state: tauri::State<'_, AppState>,
    snapshot_id: i64,
) -> Result<Option<BrowserSnapshot>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::snapshot_detail(db.conn(), snapshot_id).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Most used, pinned vault, search
// ---------------------------------------------------------------------------

#[tauri::command]
fn top_sites(
    state: tauri::State<'_, AppState>,
    window: String,
    metric: String,
    limit: Option<u32>,
) -> Result<Vec<query::SiteStatRow>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::top_sites(db.conn(), &window, &metric, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

#[tauri::command]
fn pinned_vault(
    state: tauri::State<'_, AppState>,
    browser: Option<String>,
) -> Result<Vec<query::PinnedSiteRow>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::list_pinned_sites(db.conn(), browser.as_deref()).map_err(|e| e.to_string())
}

/// Shared implementation for re-pinning pinned sites (tray + command).
///
/// `scope`: `"current"` restores only sites pinned in your last capture —
/// the "bring back my pins" moment. `"all"` restores everything the vault
/// has ever seen pinned, ignoring whether it is currently open. Sites whose
/// group is known recreate that group with the original name and color.
pub(crate) fn repin_impl(db: &Db, mode: RestoreMode, scope: &str) -> Result<i64, String> {
    let sites = query::list_pinned_sites(db.conn(), None).map_err(|e| e.to_string())?;
    let chosen: Vec<&query::PinnedSiteRow> = match scope {
        "all" => sites.iter().collect(),
        // Default: what is actually pinned right now.
        _ => {
            let current: Vec<&query::PinnedSiteRow> =
                sites.iter().filter(|s| s.currently_pinned).collect();
            if current.is_empty() {
                return Err(
                    "no sites are currently pinned in your browsers — \
                     use 'Re-pin everything ever pinned' to restore from history"
                        .to_string(),
                );
            }
            current
        }
    };
    if chosen.is_empty() {
        return Err("nothing pinned yet — pin something in your browser first".to_string());
    }
    let payload = serde_json::json!({
        "tabs": chosen.iter().enumerate().map(|(i, s)| serde_json::json!({
            "url": s.url,
            "pinned": true,
            "active": i == 0,
            "groupName": s.group_name,
            "groupColor": s.group_color,
        })).collect::<Vec<_>>(),
        "newWindow": mode.new_window,
        "skipOpen": mode.skip_open,
    });
    enqueue_restore(db, "repin_all", payload)
}

#[tauri::command]
fn repin_all(
    state: tauri::State<'_, AppState>,
    mode: RestoreMode,
    scope: Option<String>,
) -> Result<i64, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    repin_impl(&db, mode, scope.as_deref().unwrap_or("current"))
}

#[tauri::command]
fn restore_pinned_site(
    state: tauri::State<'_, AppState>,
    id: i64,
    mode: RestoreMode,
) -> Result<i64, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let sites = query::list_pinned_sites(db.conn(), None).map_err(|e| e.to_string())?;
    let site = sites
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| "that pinned site is not in the vault".to_string())?;
    let payload = serde_json::json!({
        "tabs": [serde_json::json!({
            "url": site.url,
            "pinned": true,
            "active": true,
            "groupName": site.group_name,
            "groupColor": site.group_color,
        })],
        "newWindow": mode.new_window,
        "skipOpen": mode.skip_open,
    });
    enqueue_restore(&db, "open_window", payload)
}

#[tauri::command]
fn global_search(
    state: tauri::State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<query::SearchHit>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    query::global_search(db.conn(), &query, limit.unwrap_or(30)).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Settings and recording controls
// ---------------------------------------------------------------------------

#[tauri::command]
fn settings_get(state: tauri::State<'_, AppState>) -> Result<Settings, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    Ok(db.settings())
}

fn set_autostart_impl(enable: bool) {
    #[cfg(windows)]
    {
        use std::process::Command;
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => return,
        };
        let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
        let value = format!("\"{}\"", exe.display());
        let args: Vec<&str> = if enable {
            vec!["add", key, "/v", "TabVault", "/t", "REG_SZ", "/d", &value, "/f"]
        } else {
            vec!["delete", key, "/v", "TabVault", "/f"]
        };
        let _ = Command::new("reg.exe").args(args).output();
    }
}

#[tauri::command]
fn settings_set(state: tauri::State<'_, AppState>, settings: Settings) -> Result<(), String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    db.set_settings(&settings).map_err(|e| e.to_string())?;
    // Best-effort: keep the Windows Run key in sync with the toggle.
    set_autostart_impl(settings.start_with_windows);
    Ok(())
}

#[tauri::command]
fn pause_toggle(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let mut settings = db.settings();
    settings.recording_paused = !settings.recording_paused;
    db.set_settings(&settings).map_err(|e| e.to_string())?;
    Ok(settings.recording_paused)
}

#[tauri::command]
fn add_sensitive_exclusions(state: tauri::State<'_, AppState>) -> Result<Vec<String>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let mut settings = db.settings();
    let mut seen = std::collections::BTreeSet::new();
    let mut merged: Vec<String> = Vec::new();
    for entry in settings
        .exclusions
        .iter()
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
    {
        if seen.insert(entry.clone()) {
            merged.push(entry);
        }
    }
    for pattern in tv_core::url_util::SENSITIVE_HOST_PATTERNS {
        let entry = pattern.to_ascii_lowercase();
        if seen.insert(entry.clone()) {
            merged.push(entry);
        }
    }
    settings.exclusions = merged;
    db.set_settings(&settings).map_err(|e| e.to_string())?;
    Ok(settings.exclusions)
}

#[tauri::command]
fn retention_run(state: tauri::State<'_, AppState>) -> Result<std::collections::BTreeMap<String, usize>, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    write::apply_retention(&db, tv_core::time::now_ms()).map_err(|e| e.to_string())
}

#[tauri::command]
fn retention_preview(state: tauri::State<'_, AppState>) -> Result<(usize, usize, usize, usize), String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    write::preview_retention(&db, tv_core::time::now_ms()).map_err(|e| e.to_string())
}

#[tauri::command]
fn backup_now(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let dir = vault_path()?
        .parent()
        .map(|p| p.join("backups"))
        .ok_or_else(|| "vault path has no parent".to_string())?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dest = dir.join(format!("tabvault-{stamp}.db"));
    write::backup_database(&db, &dest)
        .map(|p| p.display().to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    // Opens the URL in the user's default browser.
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", &url])
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn exports_dir() -> Result<PathBuf, String> {
    let dir = vault_path()?
        .parent()
        .map(|p| p.join("exports"))
        .ok_or_else(|| "vault path has no parent".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[tauri::command]
fn export_json(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = exports_dir()?.join(format!("tabvault-export-{stamp}.json"));
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT url, title, favicon, pinned, tab_index, browser, profile, opened_ms,
                    closed_ms, group_name, group_color, active_ms, visit_count, source,
                    confidence, window_event_id
             FROM closed_tab_events ORDER BY closed_ms DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            let url: String = r.get(0)?;
            Ok(serde_json::json!({
                "url": url,
                "title": r.get::<_, String>(1)?,
                "favicon": r.get::<_, Option<String>>(2)?,
                "pinned": r.get::<_, i64>(3)? != 0,
                "tabIndex": r.get::<_, i64>(4)?,
                "browser": r.get::<_, String>(5)?,
                "profile": r.get::<_, String>(6)?,
                "openedMs": r.get::<_, Option<i64>>(7)?,
                "closedMs": r.get::<_, i64>(8)?,
                "groupName": r.get::<_, Option<String>>(9)?,
                "groupColor": r.get::<_, Option<String>>(10)?,
                "activeMs": r.get::<_, i64>(11)?,
                "visitCount": r.get::<_, i64>(12)?,
                "source": r.get::<_, String>(13)?,
                "confidence": r.get::<_, String>(14)?,
                "windowEventId": r.get::<_, Option<i64>>(15)?,
            }))
        })
        .map_err(|e| e.to_string())?;
    let items: Vec<serde_json::Value> = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?;
    drop(stmt);

    let windows: Vec<serde_json::Value> = {
        let mut stmt = db
            .conn()
            .prepare(
                "SELECT id, browser, profile, closed_ms, tab_count, pinned_count, source, window_id
                 FROM closed_window_events ORDER BY closed_ms DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(serde_json::json!({
                    "id": r.get::<_, i64>(0)?,
                    "browser": r.get::<_, String>(1)?,
                    "profile": r.get::<_, String>(2)?,
                    "closedMs": r.get::<_, i64>(3)?,
                    "tabCount": r.get::<_, i64>(4)?,
                    "pinnedCount": r.get::<_, i64>(5)?,
                    "source": r.get::<_, String>(6)?,
                    "windowId": r.get::<_, String>(7)?,
                }))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|e| e.to_string())?
    };

    let payload = serde_json::json!({
        "format": "tabvault-export",
        "version": 1,
        "exportedAt": chrono::Utc::now().to_rfc3339(),
        "stats": {
            "closedTabs": items.len(),
            "closedWindows": windows.len(),
        },
        "closedTabs": items,
        "closedWindows": windows,
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&payload).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command]
fn export_html(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = exports_dir()?.join(format!("tabvault-bookmarks-{stamp}.html"));
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT url, title, closed_ms FROM closed_tab_events ORDER BY closed_ms DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(|e| e.to_string())?;

    let mut html = String::from(
        "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>TabVault Export</TITLE>\n<H1>TabVault Export</H1>\n<DL><p>\n    <DT><H3>Recently Closed</H3>\n    <DL><p>\n",
    );
    for row in rows {
        let (url, title, closed_ms) = row.map_err(|e| e.to_string())?;
        let title = if title.is_empty() { &url } else { &title };
        let safe_title = html_escape(title);
        let safe_url = html_escape(&url);
        html.push_str(&format!(
            "        <DT><A HREF=\"{safe_url}\" ADD_DATE=\"{}\">{safe_title}</A>\n",
            closed_ms / 1000
        ));
    }
    html.push_str("    </DL><p>\n</DL><p>\n");
    std::fs::write(&path, html).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[tauri::command]
fn export_open_folder() -> Result<(), String> {
    let dir = exports_dir()?;
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn import_seed(state: tauri::State<'_, AppState>) -> Result<ImportSeedOutcome, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let dir = exports_dir()?;
    let path = dir.join("tabvault-import.json");
    if !path.is_file() {
        return Err(format!(
            "Put your seed file at {} and try again.\nFormat: {{\"format\":\"tabvault-export\",\"version\":1,\"closedTabs\":[{{\"url\":\"...\",\"title\":\"...\",\"pinned\":false,...}}]}}\n(Any exported tabvault-export JSON works too.)",
            path.display()
        ));
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| format!("not valid JSON: {e}"))?;
    let tabs = value
        .get("closedTabs")
        .and_then(|t| t.as_array())
        .cloned()
        .ok_or_else(|| "seed has no \"closedTabs\" array".to_string())?;

    let batch_id = write::begin_import_batch(db, "seed_json", Some(&path), Some("seed"), Some("seed"))
        .map_err(|e| e.to_string())?;
    let mut found = 0usize;
    let mut imported = 0usize;
    for entry in tabs {
        found += 1;
        let url = entry.get("url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if url.is_empty() {
            continue;
        }
        let mut tab: TabRecord = serde_json::from_value(entry).unwrap_or_else(|_| TabRecord {
            url: url.clone(),
            title: String::new(),
            ..Default::default()
        });
        tab.source = tv_core::model::Source::Import;
        tab.confidence = tv_core::model::Confidence::Medium;
        let closed_ms = tab.closed_ms.or(Some(tv_core::time::now_ms()));
        let stats = write::import_recovered_tabs(db, batch_id, "seed_json", "seed", "seed", &[(tab, closed_ms)], false)
            .map_err(|e| e.to_string())?;
        imported += stats.items_imported;
    }
    write::finish_import_batch(db, batch_id, None).map_err(|e| e.to_string())?;
    Ok(ImportSeedOutcome {
        items_found: found,
        items_imported: imported,
        batch_id,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSeedOutcome {
    pub items_found: usize,
    pub items_imported: usize,
    pub batch_id: i64,
}

#[tauri::command]
fn set_encryption(state: tauri::State<'_, AppState>, enable: bool) -> Result<bool, String> {
    let db_path = vault_path()?;
    // Idempotent: already in the desired state → no-op success.
    if tv_core::db::encryption::is_encrypted(&db_path) == enable {
        return Ok(enable);
    }

    let mut guard = state.db.lock().map_err(|_| "database lock poisoned".to_string())?;

    // Take the live connection out of the state and drop it, so the migration
    // can rename the file. The migration is built to leave the vault intact on
    // any failure before the final swap; after the swap the new file has been
    // verified, so reopening it here is safe.
    let old_db = guard.take().ok_or("the vault is unavailable")?;
    drop(old_db);

    let outcome = if enable {
        tv_core::db::encryption::enable_encryption(&db_path)
    } else {
        tv_core::db::encryption::disable_encryption(&db_path)
    };

    match &outcome {
        Ok(()) => {
            let new_db = open_vault(&db_path).map_err(|e| {
                tracing::error!("failed to reopen the vault after encryption migration: {e}");
                e.to_string()
            })?;
            *guard = Some(new_db);
            Ok(enable)
        }
        Err(e) => {
            // Migration failed; it leaves either the plaintext vault or the
            // intact DPAPI-keyed encrypted vault in place by its own contract.
            // Reopen whatever is at db_path now so the app keeps working.
            tracing::warn!("encryption migration failed: {e}");
            match open_vault(&db_path) {
                Ok(new_db) => {
                    *guard = Some(new_db);
                    Err(e.to_string())
                }
                Err(e2) => {
                    tracing::error!("could not reopen the vault after a failed migration: {e2}");
                    Err(format!("{e}; additionally, the vault could not be reopened: {e2}"))
                }
            }
        }
    }
}

#[tauri::command]
fn open_data_folder() -> Result<(), String> {
    let dir = vault_path()?.parent().map(|p| p.to_path_buf()).ok_or_else(|| "vault path has no parent".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(&dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

mod commands_import;

use commands_import::{detect_profiles, import_history_file, import_session_file};

// ---------------------------------------------------------------------------
// Tray and global hotkey
// ---------------------------------------------------------------------------

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn toggle_pause_from_ui(_app: &tauri::AppHandle) -> bool {
    let Ok(path) = vault_path() else {
        return false;
    };
    let Ok(db) = open_vault(&path) else {
        return false;
    };
    let mut settings = db.settings();
    settings.recording_paused = !settings.recording_paused;
    let _ = db.set_settings(&settings);
    settings.recording_paused
}

#[cfg(windows)]
fn spawn_hotkey_thread(tx: std::sync::mpsc::Sender<()>) {
    std::thread::spawn(move || {
        // Ctrl+Alt+R: restore the most recently closed window.
        unsafe {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
            use windows_sys::Win32::UI::WindowsAndMessaging::*;
            use windows_sys::Win32::Foundation::GetLastError;

            const HOTKEY_ID: i32 = 0x5456; // "TV"
            const WM_HOTKEY: u32 = 0x0312;
            if RegisterHotKey(std::ptr::null_mut(), HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 0x52 /* 'R' */) == 0 {
                eprintln!("[tabvault] could not register Ctrl+Alt+R (error {})", GetLastError());
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                if msg.message == WM_HOTKEY && msg.wParam == HOTKEY_ID as usize {
                    let _ = tx.send(());
                }
            }
            UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID);
        }
    });
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Open (and migrate) the vault at startup so problems surface at
            // boot rather than on first UI interaction.
            let path = vault_path().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            let db = open_vault(&path)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            let _ = db.integrity_check();
            app.manage(AppState {
                db: Mutex::new(Some(db)),
            });

            // Auto-backup: check hourly; back up when the last copy is older
            // than 24h, then prune to `backup_keep` copies. Runs on its own
            // connection so the UI thread never blocks on I/O.
            std::thread::spawn(|| {
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                    let path = match vault_path() {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let db = match open_vault(&path) {
                        Ok(d) => d,
                        Err(_) => continue,
                    };
                    let settings = db.settings();
                    if !settings.daily_backup {
                        continue;
                    }
                    let last: i64 = db
                        .conn()
                        .query_row(
                            "SELECT COALESCE(MAX(CAST(value AS INTEGER)), 0) FROM settings WHERE key = 'last_backup_ms'",
                            [],
                            |r| r.get::<_, i64>(0),
                        )
                        .unwrap_or(0);
                    let now = tv_core::time::now_ms();
                    if now - last < 24 * 3600 * 1000 {
                        continue;
                    }
                    let backup_dir = path.parent().map(|p| p.join("backups")).unwrap_or_else(|| PathBuf::from("."));
                    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                    let dest = backup_dir.join(format!("tabvault-{stamp}.db"));
                    if write::backup_database(&db, &dest).is_ok() {
                        let _ = db.set_setting("last_backup_ms", &now);
                        // Prune old backups, keeping the newest `backup_keep`.
                        if let Ok(entries) = std::fs::read_dir(&backup_dir) {
                            let mut files: Vec<_> = entries
                                .filter_map(|e| e.ok())
                                .filter(|e| e.file_name().to_string_lossy().starts_with("tabvault-"))
                                .collect();
                            files.sort_by_key(|e| {
                                e.metadata()
                                    .and_then(|m| m.modified())
                                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                            });
                            let keep = settings.backup_keep as usize;
                            while files.len() > keep {
                                if let Some(oldest) = files.first() {
                                    let _ = std::fs::remove_file(oldest.path());
                                    files.remove(0);
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                }
            });

            // Tray icon with the essentials.
            let open_item = tauri::menu::MenuItemBuilder::with_id("open", "Open TabVault").build(app)?;
            let recent_item = tauri::menu::MenuItemBuilder::with_id("recent", "Recently Closed").build(app)?;
            let restore_item = tauri::menu::MenuItemBuilder::with_id("restore_last", "Restore Last Closed Window (Ctrl+Alt+R)").build(app)?;
            let repin_item = tauri::menu::MenuItemBuilder::with_id("restore_pins", "Re-pin current pinned tabs").build(app)?;
            let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
            let pause_item = tauri::menu::MenuItemBuilder::with_id("pause", "Pause Recording").build(app)?;
            let quit_item = tauri::menu::MenuItemBuilder::with_id("quit", "Quit TabVault").build(app)?;
            let menu = tauri::menu::MenuBuilder::new(app)
                .items(&[&open_item, &recent_item, &restore_item, &repin_item, &sep, &pause_item, &quit_item])
                .build()?;

            let tray = tauri::tray::TrayIconBuilder::with_id("main")
                .menu(&menu)
                .tooltip("TabVault — your tabs are protected")
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "open" | "recent" => show_main_window(app),
                        "restore_last" => {
                            let _ = (|| {
                                let path = vault_path().ok()?;
                                let db = open_vault(&path).ok()?;
                                restore_last_closed_window_impl(&db).ok()?;
                                Some(())
                            })();
                        }
                        "restore_pins" => {
                            let _ = (|| {
                                let path = vault_path().ok()?;
                                let db = open_vault(&path).ok()?;
                                repin_impl(
                                    &db,
                                    RestoreMode::default(),
                                    "current",
                                )
                                .ok()?;
                                Some(())
                            })();
                        }
                        "pause" => {
                            let paused = toggle_pause_from_ui(app);
                            if let Some(item) = app.menu().and_then(|m| m.get("pause")) {
                                let _ = item.as_menuitem().map(|i| {
                                    i.set_text(if paused { "Resume Recording" } else { "Pause Recording" })
                                });
                            }
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle().clone();
                        show_main_window(&app);
                    }
                })
                .build(app)?;
            let _ = tray;

            // System-wide Ctrl+Alt+R hotkey drives the restore too.
            #[cfg(windows)]
            {
                let (tx, rx) = std::sync::mpsc::channel::<()>();
                spawn_hotkey_thread(tx);
                std::thread::spawn(move || {
                    while rx.recv().is_ok() {
                        let path = match vault_path() {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        if let Ok(db) = open_vault(&path) {
                            let _ = restore_last_closed_window_impl(&db);
                        }
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            dashboard,
            import_batches,
            browser_profiles,
            recently_closed,
            closed_windows,
            closed_window_tabs,
            restore_tabs,
            restore_window,
            restore_snapshot,
            restore_last_closed_window,
            restore_pinned_site,
            recent_restore_requests,
            timeline,
            snapshot_detail,
            top_sites,
            pinned_vault,
            repin_all,
            global_search,
            settings_get,
            settings_set,
            pause_toggle,
            add_sensitive_exclusions,
            retention_run,
            retention_preview,
            backup_now,
            open_data_folder,
            export_json,
            export_html,
            export_open_folder,
            import_seed,
            set_encryption,
            open_url,
            detect_profiles,
            import_session_file,
            import_history_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running TabVault");
}

