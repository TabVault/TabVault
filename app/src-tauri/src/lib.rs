//! TabVault desktop app: the tray host, the WebView UI, and every window the
//! user sees. All heavy work (parsing, retention, restore enqueues) happens in
//! `tv-core`; this module only wires it to the UI.

use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::Manager;
use tv_core::db::{open_vault, query, write, Db};
use tv_core::model::{BrowserSnapshot, Settings, TabRecord};

pub struct AppState {
    pub db: Mutex<Db>,
}

fn vault_path() -> Result<PathBuf, String> {
    if let Some(override_path) = std::env::var_os("TABVAULT_DB") {
        return Ok(PathBuf::from(override_path));
    }
    let appdata = std::env::var_os("APPDATA").ok_or_else(|| "APPDATA is not set".to_string())?;
    Ok(PathBuf::from(appdata).join("TabVault").join("tabvault.db"))
}

fn db<'a>(state: &'a tauri::State<'_, AppState>) -> Result<std::sync::MutexGuard<'a, Db>, String> {
    state.db.lock().map_err(|_| "database lock poisoned".to_string())
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
        .map(|t| {
            serde_json::json!({
                "url": t.url,
                "pinned": t.pinned,
                "active": false,
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
    let db = db(&state)?;
    query::dashboard_stats(db.conn()).map_err(|e| e.to_string())
}

#[tauri::command]
fn browser_profiles(state: tauri::State<'_, AppState>) -> Result<Vec<query::BrowserProfileRow>, String> {
    let db = db(&state)?;
    query::browser_profiles(db.conn()).map_err(|e| e.to_string())
}

#[tauri::command]
fn recently_closed(
    state: tauri::State<'_, AppState>,
    filter: query::ClosedTabFilter,
) -> Result<query::ClosedTabPage, String> {
    let db = db(&state)?;
    query::list_closed_tabs(db.conn(), &filter).map_err(|e| e.to_string())
}

#[tauri::command]
fn closed_windows(
    state: tauri::State<'_, AppState>,
    browser: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<query::ClosedWindowRow>, String> {
    let db = db(&state)?;
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
    let db = db(&state)?;
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
    let db = db(&state)?;
    let conn = db.conn();
    let mut tabs = Vec::with_capacity(ids.len());
    for id in ids {
        let row = conn
            .query_row(
                "SELECT id, url, title, favicon, pinned, browser, profile, opened_ms,
                        closed_ms, last_active_ms, active_ms, visit_count, group_name,
                        group_color, window_event_id
                 FROM closed_tab_events WHERE id = ?1",
                [id],
                |r| {
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
                    })
                },
            )
            .map_err(|e| format!("closed tab {id} not found: {e}"))?;
        tabs.push(tab_from_closed_row(&row));
    }
    let payload = restore_payload_for_tabs(&tabs, mode);
    enqueue_restore(&db, "open_window", payload)
}

#[tauri::command]
fn restore_window(
    state: tauri::State<'_, AppState>,
    window_event_id: i64,
    mode: RestoreMode,
) -> Result<i64, String> {
    let db = db(&state)?;
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
    let db = db(&state)?;
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
    let db = db(&state)?;
    restore_last_closed_window_impl(&db)
}

#[tauri::command]
fn recent_restore_requests(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<(i64, String, String, String, Option<i64>)>, String> {
    let db = db(&state)?;
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
    let db = db(&state)?;
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
    let db = db(&state)?;
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
    let db = db(&state)?;
    query::top_sites(db.conn(), &window, &metric, limit.unwrap_or(50)).map_err(|e| e.to_string())
}

#[tauri::command]
fn pinned_vault(
    state: tauri::State<'_, AppState>,
    browser: Option<String>,
) -> Result<Vec<query::PinnedSiteRow>, String> {
    let db = db(&state)?;
    query::list_pinned_sites(db.conn(), browser.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
fn repin_all(
    state: tauri::State<'_, AppState>,
    mode: RestoreMode,
) -> Result<i64, String> {
    let db = db(&state)?;
    let sites = query::list_pinned_sites(db.conn(), None).map_err(|e| e.to_string())?;
    if sites.is_empty() {
        return Err("nothing pinned yet — pin something in your browser first".to_string());
    }
    // Prefer currently-pinned sites; fall back to the full vault when there is none.
    let current: Vec<&query::PinnedSiteRow> = sites.iter().filter(|s| s.currently_pinned).collect();
    let chosen: Vec<&query::PinnedSiteRow> = if current.is_empty() {
        sites.iter().collect()
    } else {
        current
    };
    let payload = serde_json::json!({
        "tabs": chosen.iter().map(|s| serde_json::json!({
            "url": s.url,
            "pinned": true,
            "active": false,
            "groupName": s.group_name,
            "groupColor": null,
        })).collect::<Vec<_>>(),
        "newWindow": mode.new_window,
        "skipOpen": mode.skip_open,
    });
    enqueue_restore(&db, "repin_all", payload)
}

#[tauri::command]
fn global_search(
    state: tauri::State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<query::SearchHit>, String> {
    let db = db(&state)?;
    query::global_search(db.conn(), &query, limit.unwrap_or(30)).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Settings and recording controls
// ---------------------------------------------------------------------------

#[tauri::command]
fn settings_get(state: tauri::State<'_, AppState>) -> Result<Settings, String> {
    let db = db(&state)?;
    Ok(db.settings())
}

#[tauri::command]
fn settings_set(state: tauri::State<'_, AppState>, settings: Settings) -> Result<(), String> {
    let db = db(&state)?;
    db.set_settings(&settings).map_err(|e| e.to_string())
}

#[tauri::command]
fn pause_toggle(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let db = db(&state)?;
    let mut settings = db.settings();
    settings.recording_paused = !settings.recording_paused;
    db.set_settings(&settings).map_err(|e| e.to_string())?;
    Ok(settings.recording_paused)
}

#[tauri::command]
fn retention_run(state: tauri::State<'_, AppState>) -> Result<std::collections::BTreeMap<String, usize>, String> {
    let db = db(&state)?;
    write::apply_retention(&db, tv_core::time::now_ms()).map_err(|e| e.to_string())
}

#[tauri::command]
fn retention_preview(state: tauri::State<'_, AppState>) -> Result<(usize, usize, usize, usize), String> {
    let db = db(&state)?;
    write::preview_retention(&db, tv_core::time::now_ms()).map_err(|e| e.to_string())
}

#[tauri::command]
fn backup_now(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let db = db(&state)?;
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
                db: Mutex::new(db),
            });

            // Tray icon with the essentials.
            let open_item = tauri::menu::MenuItemBuilder::with_id("open", "Open TabVault").build(app)?;
            let recent_item = tauri::menu::MenuItemBuilder::with_id("recent", "Recently Closed").build(app)?;
            let restore_item = tauri::menu::MenuItemBuilder::with_id("restore_last", "Restore Last Closed Window (Ctrl+Alt+R)").build(app)?;
            let sep = tauri::menu::PredefinedMenuItem::separator(app)?;
            let pause_item = tauri::menu::MenuItemBuilder::with_id("pause", "Pause Recording").build(app)?;
            let quit_item = tauri::menu::MenuItemBuilder::with_id("quit", "Quit TabVault").build(app)?;
            let menu = tauri::menu::MenuBuilder::new(app)
                .items(&[&open_item, &recent_item, &restore_item, &sep, &pause_item, &quit_item])
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
            browser_profiles,
            recently_closed,
            closed_windows,
            closed_window_tabs,
            restore_tabs,
            restore_window,
            restore_snapshot,
            restore_last_closed_window,
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
            retention_run,
            retention_preview,
            backup_now,
            open_data_folder,
            open_url,
            detect_profiles,
            import_session_file,
            import_history_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running TabVault");
}

