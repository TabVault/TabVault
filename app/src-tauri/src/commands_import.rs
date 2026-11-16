//! "Recover from the past" commands: profile detection, session-file import,
//! and history seeding. Extracted from lib.rs for clarity.

use crate::{db, vault, AppState};
use serde::Serialize;
use tv_core::db::write;
use tv_core::model::TabRecord;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileInfo {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub modified_ms: Option<i64>,
    /// `session_tabs` | `session_session` | `firefox_sessionstore` | `chromium_history` | `firefox_places` | `other`
    pub kind: String,
    /// True when the file is OSCrypt-encrypted and cannot be read without the
    /// browser's key (the UI explains this instead of pretending).
    pub encrypted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedProfileInfo {
    pub browser: String,
    pub profile_dir: String,
    pub display_name: Option<String>,
    pub path: String,
    pub files: Vec<SessionFileInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub items_found: usize,
    pub items_imported: usize,
    /// How many of the imported tabs were pinned (the "lost pinned tabs" case).
    pub pinned_imported: usize,
    pub batch_id: i64,
    pub file: String,
    pub kind: String,
    pub warnings: Vec<String>,
}

fn session_file_kind(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("tabs_") || lower.starts_with("current tabs") || lower.starts_with("last tabs") {
        "session_tabs"
    } else if lower.starts_with("session_") || lower.starts_with("current session") || lower.starts_with("last session") {
        "session_session"
    } else if lower.starts_with("recovery.jsonlz4")
        || lower.starts_with("previous.jsonlz4")
        || lower.starts_with("sessionstore.jsonlz4")
        || lower.starts_with("recovery.baklz4")
        || lower.starts_with("upgrade.jsonlz4-")
    {
        "firefox_sessionstore"
    } else if lower == "places.sqlite" {
        "firefox_places"
    } else if lower == "history" {
        "chromium_history"
    } else {
        "other"
    }
}

fn modified_ms(path: &std::path::Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
}

fn file_info(path: &std::path::Path, encrypted: bool) -> SessionFileInfo {
    SessionFileInfo {
        path: path.display().to_string(),
        name: path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        size_bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        modified_ms: modified_ms(path),
        kind: session_file_kind(
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(""),
        )
        .to_string(),
        encrypted,
    }
}

#[tauri::command]
pub fn detect_profiles() -> Result<Vec<DetectedProfileInfo>, String> {
    let mut out = Vec::new();
    for profile in tv_core::paths::detect_all_profiles() {
        let mut files: Vec<SessionFileInfo> = Vec::new();
        match profile.browser.as_str() {
            "firefox" => {
                for f in tv_core::paths::firefox_session_files(&profile) {
                    files.push(file_info(&f, false));
                }
                if let Some(p) = tv_core::paths::firefox_places(&profile) {
                    files.push(file_info(&p, false));
                }
            }
            _ => {
                for dir in tv_core::paths::chromium_session_dirs(&profile) {
                    let encrypted_dir = dir
                        .file_name()
                        .map(|n| n.eq_ignore_ascii_case("Sessions_Encrypted"))
                        .unwrap_or(false);
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_file() {
                                let name = entry.file_name().to_string_lossy().to_string();
                                if name.starts_with("Tabs_")
                                    || name.starts_with("Session_")
                                    || name.starts_with("Current ")
                                    || name.starts_with("Last ")
                                {
                                    files.push(file_info(&path, encrypted_dir));
                                }
                            }
                        }
                    }
                }
                if let Some(p) = tv_core::paths::chromium_history(&profile) {
                    files.push(file_info(&p, false));
                }
            }
        }
        files.sort_by(|a, b| b.modified_ms.unwrap_or(0).cmp(&a.modified_ms.unwrap_or(0)));
        out.push(DetectedProfileInfo {
            browser: profile.browser.clone(),
            profile_dir: profile.profile_dir.clone(),
            display_name: profile.display_name.clone(),
            path: profile.path.display().to_string(),
            files,
        });
    }
    Ok(out)
}

/// Copies `src` into a fresh scratch dir (locked-file protocol) and returns the
/// temp dir and the copy's path. The caller deletes the dir afterwards.
fn locked_copy(src: &std::path::Path) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let scratch = std::env::temp_dir().join(format!("tabvault-import-{}", tv_core::time::now_ms()));
    let copy = tv_core::locked_copy::copy_locked_db(src, &scratch).map_err(|e| e.to_string())?;
    Ok((scratch, copy.path))
}

#[tauri::command]
pub fn import_session_file(
    state: tauri::State<'_, AppState>,
    path: String,
    browser: String,
    profile: String,
) -> Result<ImportOutcome, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let file = std::path::PathBuf::from(&path);
    if !file.is_file() {
        return Err(format!("file not found: {path}"));
    }
    let name = file
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let kind_label = session_file_kind(&name).to_string();
    let (scratch, copied) = locked_copy(&file)?;

    let result = (|| -> Result<(Vec<(tv_core::parsers::snss::RecoveredWindow, Vec<TabRecord>)>, Vec<String>), String> {
        match kind_label.as_str() {
            "session_tabs" | "session_session" => {
                let kind = tv_core::parsers::snss::SessionFileKind::from_filename(&name)
                    .ok_or_else(|| format!("could not determine the kind of {name}"))?;
                let bytes = std::fs::read(&copied).map_err(|e| format!("read {}: {e}", copied.display()))?;
                let recovery = tv_core::parsers::snss::parse(&bytes, kind)
                    .map_err(|e| format!("parse {name}: {e}"))?;
                if recovery.app_bound_blobs > 0 && recovery.commands_total == 0 {
                    return Err(format!(
                        "'{name}' is encrypted with Chrome's App-Bound Encryption and cannot be read outside the browser. \
                         The cleartext mirror in the same profile (Sessions/) is what you should pick instead."
                    ));
                }
                let browser_name = browser.clone();
                let profile_name = profile.clone();
                let mut all = Vec::new();
                for w in &recovery.windows {
                    let mut tabs: Vec<TabRecord> = w
                        .tabs
                        .iter()
                        .map(|t| t.to_tab_record(&browser_name, &profile_name, true))
                        .collect();
                    tabs.sort_by_key(|t| t.tab_index);
                    for (i, t) in tabs.iter_mut().enumerate() {
                        t.tab_index = i as i64;
                    }
                    all.push((w.clone(), tabs));
                }
                Ok((all, recovery.warnings))
            }
            "firefox_sessionstore" => {
                let bytes = std::fs::read(&copied).map_err(|e| format!("read {}: {e}", copied.display()))?;
                let recovery = tv_core::parsers::firefox::parse_session_store(&bytes, &name)
                    .map_err(|e| format!("parse {name}: {e}"))?;
                let profile_name = profile.clone();
                let mut all = Vec::new();
                for w in &recovery.windows {
                    let mut tabs: Vec<TabRecord> = w
                        .tabs
                        .iter()
                        .map(|t| t.to_tab_record(&profile_name, true))
                        .collect();
                    tabs.sort_by_key(|t| t.tab_index);
                    for (i, t) in tabs.iter_mut().enumerate() {
                        t.tab_index = i as i64;
                    }
                    all.push((
                        tv_core::parsers::snss::RecoveredWindow {
                            window_id: w.window_id.clone(),
                            closed_ms: w.closed_ms,
                            tabs: Vec::new(),
                            closing: w.closing,
                        },
                        tabs,
                    ));
                }
                let loose: Vec<TabRecord> = recovery
                    .closed_tabs
                    .iter()
                    .map(|t| t.to_tab_record(&profile_name, false))
                    .collect();
                if !loose.is_empty() {
                    all.push((
                        tv_core::parsers::snss::RecoveredWindow {
                            window_id: "loose".to_string(),
                            closed_ms: None,
                            tabs: Vec::new(),
                            closing: false,
                        },
                        loose,
                    ));
                }
                Ok((all, recovery.warnings))
            }
            _ => Err(format!(
                "'{name}' is not a session file TabVault reads (expected Tabs_*, Session_* or a Firefox .jsonlz4)"
            )),
        }
    })();

    let outcome = match result {
        Ok(parsed) => parsed,
        Err(e) => {
            let _ = std::fs::remove_file(&copied);
            let _ = std::fs::remove_dir(&scratch);
            return Err(e);
        }
    };
    let (windows, warnings) = outcome;

    let batch_id = write::begin_import_batch(&db, &kind_label, Some(&file), Some(browser.as_str()), Some(profile.as_str()))
        .map_err(|e| e.to_string())?;
    let mut found = 0usize;
    let mut imported = 0usize;
    let mut pinned_imported = 0usize;
    for (window, tabs) in windows {
        found += tabs.len();
        // Skip useless entries (about:blank/newtab/extension internals).
        let tabs: Vec<TabRecord> = tabs
            .into_iter()
            .filter(|t| !tv_core::snapshot::should_skip_url(&t.url, &[]))
            .collect();
        if tabs.is_empty() {
            continue;
        }
        let closed_ms = window.closed_ms;
        pinned_imported += tabs.iter().filter(|t| t.pinned).count();
        if tabs.len() > 1 || window.closing {
            write::import_recovered_window(
                &db,
                batch_id,
                &browser,
                &profile,
                &window.window_id,
                closed_ms,
                &tabs,
                &kind_label,
            )
            .map_err(|e| e.to_string())?;
            imported += tabs.len();
        } else if let Some(tab) = tabs.into_iter().next() {
            let stats = write::import_recovered_tabs(
                &db,
                batch_id,
                &kind_label,
                &browser,
                &profile,
                &[(tab, closed_ms)],
                false,
            )
            .map_err(|e| e.to_string())?;
            imported += stats.items_imported;
        }
    }
    write::finish_import_batch(&db, batch_id, None).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&copied);
    let _ = std::fs::remove_dir(&scratch);
    Ok(ImportOutcome {
        items_found: found,
        items_imported: imported,
        pinned_imported,
        batch_id,
        file: name,
        kind: kind_label,
        warnings,
    })
}

#[tauri::command]
pub fn import_history_file(
    state: tauri::State<'_, AppState>,
    path: String,
    browser: String,
    profile: String,
) -> Result<ImportOutcome, String> {
    let guard = db(&state)?;
    let db = vault(&guard)?;
    let file = std::path::PathBuf::from(&path);
    if !file.is_file() {
        return Err(format!("file not found: {path}"));
    }
    let batch_id = write::begin_import_batch(&db, "browser_history", Some(&file), Some(&browser), Some(&profile))
        .map_err(|e| e.to_string())?;
    let outcome = (|| -> Result<usize, String> {
        // Copy the locked browser database, read it, then write to the vault —
        // never seed from the copy (it is scratch) — always seed into the vault.
        let (scratch, copied) = locked_copy(&file)?;
        let conn = rusqlite::Connection::open_with_flags(&copied, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("open {}: {e}", copied.display()))?;
        let is_firefox = browser.eq_ignore_ascii_case("firefox")
            || file
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .eq_ignore_ascii_case("places.sqlite");
        let mut count = 0usize;
        if is_firefox {
            for row in tv_core::parsers::firefox::read_places(&conn).map_err(|e| e.to_string())? {
                write::seed_visit_stats(
                    db.conn(),
                    &browser,
                    &profile,
                    &row.url,
                    &row.title,
                    row.visit_count,
                    row.last_visit_ms(),
                    row.first_visit_ms(),
                )
                .map_err(|e| e.to_string())?;
                count += 1;
            }
        } else {
            for row in tv_core::parsers::chromium_history::read_chromium_history(&conn).map_err(|e| e.to_string())? {
                write::seed_visit_stats(
                    db.conn(),
                    &browser,
                    &profile,
                    &row.url,
                    &row.title,
                    row.visit_count,
                    row.last_visit_ms,
                    None,
                )
                .map_err(|e| e.to_string())?;
                count += 1;
            }
        }
        let _ = std::fs::remove_file(&copied);
        let _ = std::fs::remove_dir(&scratch);
        Ok(count)
    })();
    match outcome {
        Ok(count) => {
            write::finish_import_batch(&db, batch_id, None).map_err(|e| e.to_string())?;
            Ok(ImportOutcome {
                items_found: count,
                items_imported: count,
                pinned_imported: 0, // history databases carry no pinned state
                batch_id,
                file: file
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                kind: "browser_history".to_string(),
                warnings: Vec::new(),
            })
        }
        Err(e) => {
            let _ = write::finish_import_batch(&db, batch_id, Some(&e));
            Err(e)
        }
    }
}
