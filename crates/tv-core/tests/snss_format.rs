//! Synthetic SNSS fixtures built by hand to encode the exact format Chromium
//! writes (see `crates/tv-core/src/parsers/pickle.rs` for the primitives and
//! `snss.rs` for the field layouts). These tests prove end-to-end parsing of
//! both command spaces without ever touching a real profile, so they stay in
//! the repo forever.
//!
//! If Chromium changes its format, these fixtures are the record of what we
//! supported, and the failures tell you exactly which command moved.

use tv_core::parsers::snss::{parse, SessionFileKind};

/// Build a full SNSS file (header + records) from id/payload pairs.
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

/// Pickle writer with a valid length header (as UpdateTabNavigation carries).
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

/// A navigation command payload, exactly as Chrome writes it:
/// {tab_id, index, url, title, page_state, transition} (tail elided).
fn navigation_payload(tab_id: i32, index: i32, url: &str, title: &str) -> Vec<u8> {
    pickle(&[
        &f_i32(tab_id),
        &f_i32(index),
        &f_string(url),
        &f_string16(title),
        &f_string(""), // encoded page state
        &f_i32(0),     // transition
    ])
}

/// kCommandWindow payload (tab restore service).
fn window_payload(
    window_id: i32,
    selected_tab_index: i32,
    num_tabs: i32,
    timestamp_windows_us: i64,
) -> Vec<u8> {
    pickle(&[
        &f_i32(window_id),
        &f_i32(selected_tab_index),
        &f_i32(num_tabs),
        &f_i64(timestamp_windows_us),
        &f_i32(0), // x
        &f_i32(0), // y
        &f_i32(1000), // width
        &f_i32(800),  // height
        &f_i32(1),    // show_state (normal)
        &f_string(""), // workspace
        &f_i32(0),    // type (normal)
    ])
}

fn f_i64(v: i64) -> impl Fn(&mut Vec<u8>) {
    move |j: &mut Vec<u8>| j.extend_from_slice(&v.to_le_bytes())
}

fn windows_micros(unix_ms: i64) -> i64 {
    (unix_ms + 11_644_473_600_000) * 1000
}

#[test]
fn tab_restore_file_with_closed_window_and_loose_tab() {
    // Chrome's tab-restore file structure:
    //   T_SELECTED_NAV_IN_TAB(4){id, index, timestamp}   <- tab 1 of window 900
    //   T_UPDATE_TAB_NAVIGATION(1){...}
    //   T_PINNED_STATE(5){true}                          <- tab 1 was pinned
    //   T_SELECTED_NAV_IN_TAB(4){...}                     <- tab 2 of window 900
    //   ... navigations ...
    //   T_WINDOW(9){window 900, num_tabs=2, closed at t}   <- closed window record
    //   T_SELECTED_NAV_IN_TAB(4){...}                     <- lone tab closed outside any window
    //   T_UPDATE_TAB_NAVIGATION(1){...}
    let closed_at = windows_micros(1_760_000_000_000);
    let bytes = snss_file(&[
        // Window command first: Chrome writes window record then its tabs.
        (9, window_payload(900, 0, 2, closed_at)),
        // Tab 1 (pinned)
        (4, {
            let mut p = p32(42);
            p.extend(p32(0));
            p.extend(p64(closed_at));
            p
        }),
        (5, vec![1]), // pinned
        (1, navigation_payload(42, 0, "https://mail.example.com/inbox", "Inbox")),
        // Tab 2 (not pinned)
        (4, {
            let mut p = p32(43);
            p.extend(p32(0));
            p.extend(p64(closed_at));
            p
        }),
        (1, navigation_payload(43, 0, "https://github.com/anthropic", "anthropic (GitHub)")),
        // Loose closed tab (no window):
        (4, {
            let mut p = p32(50);
            p.extend(p32(0));
            p.extend(p64(closed_at));
            p
        }),
        (1, navigation_payload(50, 0, "https://docs.rs/anyhow", "anyhow - Rust")),
    ]);

    let recovery = parse(&bytes, SessionFileKind::TabRestore).expect("parse");
    assert_eq!(recovery.windows.len(), 1, "exactly one closed window expected");
    let w = &recovery.windows[0];
    assert_eq!(w.window_id, "900");
    assert_eq!(w.tabs.len(), 2);
    assert!(w.tabs[0].pinned, "first tab was pinned");
    assert!(!w.tabs[1].pinned);
    assert_eq!(w.tabs[0].url, "https://mail.example.com/inbox");
    assert_eq!(w.tabs[1].url, "https://github.com/anthropic");
    assert!(w.closing);
    // Loose tab is captured separately.
    assert_eq!(recovery.closed_tabs.len(), 1);
    assert_eq!(recovery.closed_tabs[0].url, "https://docs.rs/anyhow");
}

#[test]
fn session_service_file_replays_current_state_and_closes() {
    // Session-service (Session_*) files record everything from the start of
    // the session: markers, navigations, closes. Our parser must keep the
    // closes instead of discarding them like Chrome's replay would.
    let bytes = snss_file(&[
        // Marker separating initial state from the append log. We keep both.
        (255, Vec::new()),
        (0, { // SetTabWindow{window_id=7, tab_id=100}
            let mut p = p32(7);
            p.extend(p32(100));
            p
        }),
        (2, { // SetTabIndexInWindow{tab_id=100, index=0}
            let mut p = p32(100);
            p.extend(p32(0));
            p
        }),
        (6, navigation_payload(100, 0, "https://example.com/", "Example")),
        // The user closes the tab: Chrome erases it during replay; we keep it.
        (16, { // TabClosed{id=100, close_time (Windows µs)}
            let mut p = p32(100);
            p.extend(p64(windows_micros(1_760_000_000_000)));
            p
        }),
        // A window created later, still open at file write time.
        (0, {
            let mut p = p32(8);
            p.extend(p32(101));
            p
        }),
        (2, {
            let mut p = p32(101);
            p.extend(p32(0));
            p
        }),
        (6, navigation_payload(101, 0, "https://live.example.com/", "Live")),
    ]);

    let recovery = parse(&bytes, SessionFileKind::SessionService).expect("parse");
    // The closed tab is reported as a closed event with its close time.
    assert_eq!(recovery.closed_tabs.len(), 1);
    assert_eq!(recovery.closed_tabs[0].url, "https://example.com/");
    assert!(recovery.closed_tabs[0].closed_ms.is_some());
    // The still-open window/tab survives in the live section.
    assert_eq!(recovery.windows.len(), 1);
    assert_eq!(recovery.windows[0].tabs.len(), 1);
    assert_eq!(recovery.windows[0].tabs[0].url, "https://live.example.com/");
    assert!(recovery.windows[0].closed_ms.is_none());
}

#[test]
fn window_closed_in_session_file_recovers_full_window() {
    // When Chrome records a window close, we want the whole tab set, not just
    // the command — that is what "restore the window as it was" needs.
    let close = windows_micros(1_760_000_000_000);
    let bytes = snss_file(&[
        (255, Vec::new()),
        (0, {
            let mut p = p32(77);
            p.extend(p32(500));
            p
        }),
        (2, {
            let mut p = p32(500);
            p.extend(p32(0));
            p
        }),
        (6, navigation_payload(500, 0, "https://a.example.com/", "A")),
        (0, {
            let mut p = p32(77);
            p.extend(p32(501));
            p
        }),
        (2, {
            let mut p = p32(501);
            p.extend(p32(1));
            p
        }),
        (6, navigation_payload(501, 0, "https://b.example.com/", "B")),
        // Window 77 closes with both tabs inside it.
        (17, {
            let mut p = p32(77);
            p.extend(p64(close));
            p
        }),
    ]);

    let recovery = parse(&bytes, SessionFileKind::SessionService).expect("parse");
    assert_eq!(recovery.windows.len(), 1);
    let w = &recovery.windows[0];
    assert_eq!(w.window_id, "77");
    assert!(w.closing);
    assert_eq!(w.tabs.len(), 2);
    let urls: Vec<&str> = w.tabs.iter().map(|t| t.url.as_str()).collect();
    assert!(urls.contains(&"https://a.example.com/"));
    assert!(urls.contains(&"https://b.example.com/"));
}

#[test]
fn group_metadata_and_token_are_parsed() {
    // Session SetTabGroupMetadata2 body layout:
    //   payload[..16]      = opaque 16-byte token
    //   payload[16..]      = raw (non-pickle) field stream:
    //                        string16 title, u32 color, bool is_collapsed
    let token = [7u8; 16];
    let mut payload = Vec::new();
    payload.extend_from_slice(&token);
    let title = "Work";
    let units: Vec<u16> = title.encode_utf16().collect();
    payload.extend_from_slice(&(units.len() as u32).to_le_bytes());
    for u in &units {
        payload.extend_from_slice(&u.to_le_bytes());
    }
    payload.extend_from_slice(&2u32.to_le_bytes()); // color = blue
    payload.extend_from_slice(&0u32.to_le_bytes()); // is_collapsed = false

    let bytes = snss_file(&[
        (255, Vec::new()),
        (27, payload), // SetTabGroupMetadata2
    ]);
    let recovery = parse(&bytes, SessionFileKind::SessionService).expect("parse");
    assert_eq!(recovery.groups.len(), 1, "group should be recognised");
    assert_eq!(recovery.groups[0].title, "Work");
    assert_eq!(recovery.groups[0].color, 2);
    assert_eq!(recovery.groups[0].token, token.to_vec());
}
