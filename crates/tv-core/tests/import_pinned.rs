//! Integration tests: pinned tabs recovered from real browser session formats
//! survive the import pipeline into the vault — the "lost my pinned tabs"
//! scenario. These lock the whole chain: SNSS parse → import → pinned vault.

use tv_core::db::{open_vault, write};
use tv_core::model::TabRecord;
use tv_core::parsers::snss::{parse, SessionFileKind};

fn snss_file(records: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"SNSS");
    buf.extend_from_slice(&3i32.to_le_bytes());
    for (id, payload) in records {
        let size = (payload.len() + 1) as u16;
        buf.extend_from_slice(&size.to_le_bytes());
        buf.push(*id);
        buf.extend_from_slice(payload);
    }
    buf
}

fn p32(v: i32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn p64(v: i64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

fn pickle(fields: &[&dyn Fn(&mut Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for f in fields {
        f(&mut body);
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn f_i32(v: i32) -> impl Fn(&mut Vec<u8>) {
    move |j: &mut Vec<u8>| j.extend_from_slice(&v.to_le_bytes())
}

fn f_string(s: &str) -> impl Fn(&mut Vec<u8>) + use<'_> {
    move |j: &mut Vec<u8>| {
        j.extend_from_slice(&(s.len() as u32).to_le_bytes());
        j.extend_from_slice(s.as_bytes());
        let pad = (4 - (s.len() % 4)) % 4;
        j.extend(std::iter::repeat_n(0u8, pad));
    }
}

fn f_string16(s: &str) -> impl Fn(&mut Vec<u8>) + use<'_> {
    move |j: &mut Vec<u8>| {
        let units: Vec<u16> = s.encode_utf16().collect();
        j.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for u in &units {
            j.extend_from_slice(&u.to_le_bytes());
        }
        let pad = (4 - ((units.len() * 2) % 4)) % 4;
        j.extend(std::iter::repeat_n(0u8, pad));
    }
}

fn navigation_payload(tab_id: i32, index: i32, url: &str, title: &str) -> Vec<u8> {
    pickle(&[
        &f_i32(tab_id),
        &f_i32(index),
        &f_string(url),
        &f_string16(title),
        &f_string(""),
        &f_i32(0),
    ])
}

fn windows_micros(unix_ms: i64) -> i64 {
    (unix_ms + 11_644_473_600_000) * 1000
}

fn scratch_vault(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tv-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("vault.db")
}

/// A Session_* file (live-state capture) with one pinned tab in the window.
/// Importing it must preserve the pin end-to-end.
#[test]
fn pinned_session_file_survives_import_to_vault() {
    let url = "https://pinned.example.com/";
    let bytes = snss_file(&[
        (255, Vec::new()),
        (0, {
            let mut p = p32(77);
            p.extend(p32(500));
            p
        }), // SetTabWindow{window=77, tab=500}
        (2, {
            let mut p = p32(500);
            p.extend(p32(0));
            p
        }), // SetTabIndexInWindow
        (6, navigation_payload(500, 0, url, "Pinned Page")),
        (12, {
            // SetPinnedState{tab=500, pinned=true} — 5-byte form: id(4) + bool byte(1)
            let mut p = p32(500);
            p.push(1);
            p
        }),
        (17, {
            let mut p = p32(77);
            p.extend(p64(windows_micros(1_760_000_000_000)));
            p
        }), // WindowClosed
    ]);

    let recovery = parse(&bytes, SessionFileKind::SessionService).expect("parse session file");
    assert_eq!(recovery.windows.len(), 1);
    assert!(recovery.windows[0].tabs[0].pinned);

    let vault_path = scratch_vault("pinned-session-import");
    let db = open_vault(&vault_path).unwrap();
    let batch = write::begin_import_batch(
        &db,
        "session_session",
        None,
        Some("chrome"),
        Some("Default"),
    )
    .unwrap();
    let tabs: Vec<TabRecord> = recovery.windows[0]
        .tabs
        .iter()
        .map(|t| t.to_tab_record("chrome", "Default", true))
        .collect();
    write::import_recovered_window(
        &db,
        batch,
        "chrome",
        "Default",
        &recovery.windows[0].window_id,
        recovery.windows[0].closed_ms,
        &tabs,
        "chromium_snss",
    )
    .unwrap();

    // 1. The window close event exists.
    let windows: i64 = db
        .conn()
        .query_row("SELECT count(*) FROM closed_window_events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(windows, 1);

    // 2. The closed tab event carries the pin.
    let pinned_event: i64 = db
        .conn()
        .query_row(
            "SELECT pinned FROM closed_tab_events WHERE url = ?1",
            rusqlite::params![url],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pinned_event, 1);

    // 3. The pinned vault knows it is currently pinned (the lost-pins restore path).
    let (currently_pinned, pin_count): (i64, i64) = db
        .conn()
        .query_row(
            "SELECT currently_pinned, pin_count FROM pinned_sites WHERE url = ?1",
            rusqlite::params![url],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(currently_pinned, 1);
    assert_eq!(pin_count, 1);

    let _ = std::fs::remove_dir_all(vault_path.parent().unwrap());
}

/// A Tabs_* file (recently-closed capture) with a pinned tab in a closed window.
#[test]
fn pinned_tab_restore_file_survives_import_to_vault() {
    let url = "https://pinned-restore.example.com/";
    let closed_at = windows_micros(1_760_000_000_000);
    let f900 = f_i32(900);
    let f0 = f_i32(0);
    let f1 = f_i32(1);
    let fclosed = f_i64(closed_at);
    let window_fields: Vec<&dyn Fn(&mut Vec<u8>)> = vec![&f900, &f0, &f1, &fclosed];
    let window_payload = pickle(&window_fields);

    let bytes = snss_file(&[
        (9, window_payload), // kCommandWindow
        (4, {
            let mut p = p32(42);
            p.extend(p32(0));
            p.extend(p64(closed_at));
            p
        }), // SelectedNavigationInTab
        (5, vec![1u8]),                             // PinnedState = true
        (1, navigation_payload(42, 0, url, "Pinned Lost Tab")), // UpdateTabNavigation
    ]);

    let recovery = parse(&bytes, SessionFileKind::TabRestore).expect("parse tab restore file");
    assert_eq!(recovery.windows.len(), 1);
    assert!(recovery.windows[0].tabs[0].pinned);

    let vault_path = scratch_vault("pinned-tabs-import");
    let db = open_vault(&vault_path).unwrap();
    let batch = write::begin_import_batch(
        &db,
        "session_tabs",
        None,
        Some("chrome"),
        Some("Default"),
    )
    .unwrap();
    let tabs: Vec<TabRecord> = recovery.windows[0]
        .tabs
        .iter()
        .map(|t| t.to_tab_record("chrome", "Default", true))
        .collect();
    write::import_recovered_window(
        &db,
        batch,
        "chrome",
        "Default",
        &recovery.windows[0].window_id,
        recovery.windows[0].closed_ms,
        &tabs,
        "chromium_snss",
    )
    .unwrap();

    let pinned_event: i64 = db
        .conn()
        .query_row(
            "SELECT pinned FROM closed_tab_events WHERE url = ?1",
            rusqlite::params![url],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pinned_event, 1);

    let currently_pinned: i64 = db
        .conn()
        .query_row(
            "SELECT currently_pinned FROM pinned_sites WHERE url = ?1",
            rusqlite::params![url],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(currently_pinned, 1);

    let _ = std::fs::remove_dir_all(vault_path.parent().unwrap());
}

fn f_i64(v: i64) -> impl Fn(&mut Vec<u8>) {
    move |j: &mut Vec<u8>| j.extend_from_slice(&v.to_le_bytes())
}
