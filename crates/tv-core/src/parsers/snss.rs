//! Chromium **SNSS** session-file parser.
//!
//! Chrome and Edge persist append-only files in the profile's `Sessions/`
//! directory (and, since recent builds, an OS-encrypted mirror in
//! `Sessions_Encrypted/`):
//!
//! | File | Contents | Command-id space |
//! |------|----------|------------------|
//! | `Session_<µs>` | The full state of the open browser: windows, tabs, navigations, selection, groups. In the browser, `WindowClosed`/`TabClosed` commands *erase* entries during replay. | session service |
//! | `Tabs_<µs>`    | The list of recently closed entries — the tab-restore service's persisted log. **This is what the user wants back.** | tab restore service |
//!
//! plus the pre-Chrome-85 flat files `Current Session`, `Last Session`,
//! `Current Tabs`, `Last Tabs`.
//!
//! File format (verified against the Chromium sources):
//!
//! ```text
//! Header:   "SNSS" (0x53534E53) + int32 little-endian version
//!           cleartext = 3 (with a 255 "initial state marker" command)
//!           encrypted  = 5 (each record body is OSCrypt-encrypted)
//! Record:   uint16 LE size (includes the 1 id byte, excludes these 2)
//!           uint8   command id
//!           size-1  payload bytes
//! ```
//!
//! For recovery purposes the append log is a feature, not a problem: the
//! entries the browser would discard on replay *are the lost tabs*, so we keep
//! everything and tag each record with where it came from.
//!
//! `v20` (App-Bound Encryption) blobs cannot be decrypted outside the browser
//! — we detect that and report it verbatim instead of emitting garbage. `v10`
//! blobs are decrypted when an OSCrypt key is supplied.
//!
//! This module performs **no I/O**: it parses bytes, so it is unit-tested
//! against synthetic fixtures that encode the real format exactly.

use crate::crypto::decrypt_os_crypt_v10;
use crate::error::{Error, Result};
use crate::model::{Confidence, Source, TabRecord};
use crate::parsers::pickle::PickleReader;
use crate::time::windows_micros_to_unix_ms;
use std::collections::BTreeMap;

/// Which command-id space a file uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFileKind {
    /// `Session_*` — full session state (session service).
    SessionService,
    /// `Tabs_*` — recently closed entries (tab restore service).
    TabRestore,
}

impl SessionFileKind {
    /// Classifies a file by name, including the pre-Chrome-85 flat layout.
    pub fn from_filename(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        // Windows profile root files (very old builds) use spaces.
        let normalized: String = lower.chars().map(|c| if c == ' ' { '_' } else { c }).collect();
        if normalized.starts_with("tabs_")
            || normalized == "current_tabs"
            || normalized == "last_tabs"
        {
            Some(SessionFileKind::TabRestore)
        } else if normalized.starts_with("session_")
            || normalized == "current_session"
            || normalized == "last_session"
        {
            Some(SessionFileKind::SessionService)
        } else {
            None
        }
    }
}

/// The outcome of parsing one SNSS file. Never fails wholesale on a single
/// bad record: unrecognised/broken commands are counted in `skipped_commands`
/// and reported in `warnings`, so a partially-corrupt file still yields tabs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recovery {
    /// Windows reconstructed from the file. `closed_ms` set means the file
    /// recorded the window as closed; `None` means it was the live state.
    pub windows: Vec<RecoveredWindow>,
    /// Closed tabs that are not attached to a window record (single closes).
    pub closed_tabs: Vec<RecoveredTab>,
    /// Tab groups seen in the file.
    pub groups: Vec<RecoveredGroup>,
    /// Unparsed command ids and how often they occurred.
    pub skipped_commands: BTreeMap<u8, usize>,
    /// Human-readable diagnostics.
    pub warnings: Vec<String>,
    /// Total records parsed (before any decryption failures).
    pub commands_total: usize,
    /// The SNSS header version (1, 3 or 5).
    pub version: i32,
    /// Number of `v20` blobs seen (app-bound, cannot decrypt).
    pub app_bound_blobs: usize,
}

/// A window reconstructed from the append log.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecoveredWindow {
    pub window_id: String,
    /// When the file recorded the close, in Unix milliseconds.
    pub closed_ms: Option<i64>,
    pub tabs: Vec<RecoveredTab>,
    /// True if a close command recorded this window as closed.
    pub closing: bool,
}

/// A group seen in the append log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredGroup {
    /// Opaque 16-byte token; compared by equality only.
    pub token: Vec<u8>,
    pub title: String,
    /// Chrome's int encoding of the group color.
    pub color: i64,
    pub collapsed: bool,
}

/// A single navigation entry, as reconstructed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecoveredTab {
    pub url: String,
    pub title: String,
    pub pinned: bool,
    /// The session-file-internal tab id (used to correlate, not wall-clock).
    pub tab_session_id: i32,
    pub window_id: String,
    pub tab_index: i64,
    /// Raw group token as written in the file, resolved to title/color below.
    pub group_token: Option<Vec<u8>>,
    /// Resolved group title/color, when the file carried group metadata.
    pub group_name: Option<String>,
    pub group_color: Option<String>,
    pub closed_ms: Option<i64>,
    pub last_active_ms: Option<i64>,
    pub confidence: Confidence,
}

impl RecoveredTab {
    pub fn to_tab_record(
        &self,
        browser: &str,
        profile: &str,
        part_of_closed_window: bool,
    ) -> TabRecord {
        TabRecord {
            url: self.url.clone(),
            title: self.title.clone(),
            favicon: None,
            pinned: self.pinned,
            tab_index: self.tab_index,
            window_id: self.window_id.clone(),
            group_name: self.group_name.clone(),
            group_color: self.group_color.clone(),
            browser: browser.to_string(),
            profile: profile.to_string(),
            opened_ms: None,
            closed_ms: self.closed_ms,
            last_active_ms: self.last_active_ms,
            active_ms: 0,
            visit_count: 0,
            part_of_closed_window,
            incognito: false,
            source: Source::ChromiumSnss,
            confidence: self.confidence,
        }
    }
}

// ---------------------------------------------------------------------------
// Command ids — session service (Session_*)
// ---------------------------------------------------------------------------
const S_SET_TAB_WINDOW: u8 = 0;
const S_SET_TAB_INDEX_IN_WINDOW: u8 = 2;
const S_UPDATE_TAB_NAVIGATION: u8 = 6;
const S_SET_SELECTED_NAVIGATION_INDEX: u8 = 7;
const S_SET_SELECTED_TAB_IN_INDEX: u8 = 8;
const S_SET_WINDOW_TYPE: u8 = 9;
const S_SET_PINNED_STATE: u8 = 12;
const S_SET_WINDOW_BOUNDS_3: u8 = 14;
const S_TAB_CLOSED: u8 = 16;
const S_WINDOW_CLOSED: u8 = 17;
const S_SET_ACTIVE_WINDOW: u8 = 20;
const S_LAST_ACTIVE_TIME: u8 = 21;
const S_NAVIGATION_PATH_PRUNED: u8 = 24;
const S_SET_TAB_GROUP: u8 = 25;
const S_SET_TAB_GROUP_METADATA_2: u8 = 27;
const S_INITIAL_STATE_MARKER: u8 = 255;

// ---------------------------------------------------------------------------
// Command ids — tab restore service (Tabs_*)
// ---------------------------------------------------------------------------
const T_UPDATE_TAB_NAVIGATION: u8 = 1;
const T_RESTORED_ENTRY: u8 = 2;
const T_WINDOW_DEPRECATED: u8 = 3;
const T_SELECTED_NAV_IN_TAB: u8 = 4;
const T_PINNED_STATE: u8 = 5;
const T_SET_EXTENSION_APP_ID: u8 = 6;
const T_SET_WINDOW_APP_NAME: u8 = 7;
const T_SET_WINDOW_USER_TITLE: u8 = 12;
const T_WINDOW: u8 = 9;
const T_TAB_GROUP_DATA: u8 = 10;
const T_CREATE_GROUP: u8 = 13;
const T_ADD_TAB_EXTRA_DATA: u8 = 14;
const T_CREATE_SPLIT: u8 = 15;
const T_SET_SPLIT_DATA: u8 = 16;

/// A raw on-disk record after framing is undone.
#[derive(Debug)]
struct Record {
    id: u8,
    payload: Vec<u8>,
}

/// Splits cleartext SNSS bytes into raw records, noting truncation in warnings.
///
/// `body` must already have the 8-byte header removed.
fn cleartext_records(body: &[u8], warnings: &mut Vec<String>) -> Vec<Record> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 2 <= body.len() {
        let size = u16::from_le_bytes([body[pos], body[pos + 1]]) as usize;
        if size == 0 {
            warnings.push("zero-length record at the tail of the file (half-written append log; browser crash?)".into());
            break;
        }
        if pos + 2 + size > body.len() {
            warnings.push(format!(
                "truncated final record; {} byte(s) after offset {} were ignored",
                body.len() - pos,
                pos
            ));
            break;
        }
        out.push(Record {
            id: body[pos + 2],
            payload: body[pos + 3..pos + 2 + size].to_vec(),
        });
        pos += 2 + size;
    }
    out
}

/// Splits a v5 (encrypted) append log into raw records, decrypting each blob
/// with `key`.
///
/// Layout: `uint32 LE encrypted_size` + `blob[encrypted_size]`, where blob =
/// `"v10" || nonce(12) || ciphertext+tag`, plaintext = `id || payload`.
/// App-bound (`v20`) blobs are counted in `app_bound` and skipped.
fn encrypted_records(
    body: &[u8],
    base: u64,
    key: &[u8; 32],
    app_bound: &mut usize,
    warnings: &mut Vec<String>,
) -> Vec<Record> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 4 <= body.len() {
        let size = u32::from_le_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]) as usize;
        if size == 0 {
            break;
        }
        if pos + 4 + size > body.len() {
            warnings.push(format!(
                "truncated encrypted record at offset {}; remaining bytes ignored",
                base + pos as u64
            ));
            break;
        }
        let blob = &body[pos + 4..pos + 4 + size];
        if blob.starts_with(b"v20") {
            *app_bound += 1;
        } else if blob.starts_with(b"v10") {
            match decrypt_os_crypt_v10(key, blob) {
                Ok(plain) if !plain.is_empty() => {
                    out.push(Record {
                        id: plain[0],
                        payload: plain[1..].to_vec(),
                    });
                }
                Ok(_) => {
                    warnings.push(format!(
                        "empty decrypted record at offset {} (key mismatch?)",
                        base + pos as u64
                    ));
                }
                Err(e) => {
                    warnings.push(format!(
                        "failed to decrypt record at offset {}: {e}",
                        base + pos as u64
                    ));
                }
            }
        } else {
            warnings.push(format!(
                "unrecognised encrypted blob at offset {} (does not start with v10/v20)",
                base + pos as u64
            ));
        }
        pos += 4 + size;
    }
    out
}

/// Validates the SNSS header. Returns `(version, header_len)`.
fn header(bytes: &[u8]) -> Result<(i32, usize)> {
    if bytes.len() < 8 {
        return Err(Error::BadFormat("SNSS file is shorter than its 8-byte header".into()));
    }
    if !bytes.starts_with(b"SNSS") {
        return Err(Error::BadFormat("missing SNSS magic (expected 'SNSS')".into()));
    }
    let version = i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    match version {
        1 | 3 | 5 => Ok((version, 8)),
        v => Err(Error::Unsupported(format!(
            "SNSS version {v} is not supported (1, 3 and 5 are)"
        ))),
    }
}

/// Parses a cleartext SNSS file (v1 or v3).
///
/// Returns `Error::EncryptedSessionFile` for v5 — call [`parse_encrypted_v5`].
pub fn parse(bytes: &[u8], kind: SessionFileKind) -> Result<Recovery> {
    let (version, header_len) = header(bytes)?;
    if version == 5 {
        return Err(Error::EncryptedSessionFile { version });
    }
    let mut warnings = Vec::new();
    let records = cleartext_records(&bytes[header_len..], &mut warnings);
    let mut recovery = finish(records, kind, version, 0);
    recovery.warnings = warnings;
    Ok(recovery)
}

/// Parses a v5 (OSCrypt-encrypted) SNSS file using `key` (a 32-byte OSCrypt
/// key, e.g. from [`crate::crypto::unwrap_os_crypt_key`]).
///
/// `v20` (app-bound) blobs are counted but cannot be decrypted; when none of
/// the blobs can be decrypted the returned [`Recovery`] reports that explicitly
/// instead of fabricating entries.
pub fn parse_encrypted_v5(bytes: &[u8], kind: SessionFileKind, key: &[u8; 32]) -> Result<Recovery> {
    let (version, header_len) = header(bytes)?;
    if version != 5 {
        // Happy path delegate for cleartext files.
        return parse(bytes, kind);
    }
    let mut warnings = Vec::new();
    let mut app_bound = 0usize;
    let records = encrypted_records(&bytes[header_len..], header_len as u64, key, &mut app_bound, &mut warnings);
    let mut recovery = finish(records, kind, version, app_bound);
    recovery.warnings = warnings;
    if recovery.commands_total == 0 && app_bound > 0 {
        recovery.warnings.push(format!(
            "this file is encrypted with Chrome's App-Bound Encryption ({app_bound} v20 blobs); \
             it can only be unlocked by the browser itself. The cleartext mirror in the same profile \
             (Sessions/) is read instead — see the README."
        ));
    }
    Ok(recovery)
}

/// Runs the per-kind state machine.
fn finish(records: Vec<Record>, kind: SessionFileKind, version: i32, app_bound: usize) -> Recovery {
    let mut out = Recovery {
        commands_total: records.len(),
        version,
        app_bound_blobs: app_bound,
        ..Default::default()
    };
    match kind {
        SessionFileKind::SessionService => parse_session_records(&records, &mut out),
        SessionFileKind::TabRestore => parse_tab_restore_records(&records, &mut out),
    }
    // Attach confidence to everything we produced.
    // Confidence: a tab whose navigation URL survived is always usable to
    // *reopen*; richer details depend on how much else survived.
    for t in &mut out.closed_tabs {
        t.confidence = if t.url.is_empty() {
            Confidence::Low
        } else if t.closed_ms.is_some() {
            Confidence::High
        } else {
            Confidence::Medium
        };
    }
    for w in &mut out.windows {
        for t in &mut w.tabs {
            t.confidence = if t.url.is_empty() {
                Confidence::Low
            } else if t.closed_ms.is_some() {
                Confidence::High
            } else {
                Confidence::Medium
            };
        }
    }
    // Resolve group tokens to titles/colors everywhere at once, now that the
    // full token set is known.
    let group_lookup: BTreeMap<Vec<u8>, RecoveredGroup> = out
        .groups
        .iter()
        .map(|g| (g.token.clone(), g.clone()))
        .collect();
    let resolve = |t: &mut RecoveredTab| {
        if let Some(tok) = &t.group_token {
            if let Some(g) = group_lookup.get(tok) {
                if t.group_name.is_none() {
                    t.group_name = Some(g.title.clone());
                }
                if t.group_color.is_none() {
                    t.group_color = Some(g.color.to_string());
                }
            }
        }
    };
    for t in &mut out.closed_tabs {
        resolve(t);
    }
    for w in &mut out.windows {
        for t in &mut w.tabs {
            resolve(t);
        }
    }
    out
}

fn note_skipped(out: &mut Recovery, rec: &Record) {
    *out.skipped_commands.entry(rec.id).or_insert(0) += 1;
}

fn read_i32_le(payload: &[u8], at: usize) -> Option<i32> {
    let slice = payload.get(at..at.checked_add(4)?)?;
    Some(i32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_i64_le(payload: &[u8], at: usize) -> Option<i64> {
    let slice = payload.get(at..at.checked_add(8)?)?;
    let mut arr = [0u8; 8];
    arr.copy_from_slice(slice);
    Some(i64::from_le_bytes(arr))
}

fn read_token16(payload: &[u8], at: usize) -> Option<Vec<u8>> {
    payload.get(at..at.checked_add(16)?).map(|s| s.to_vec())
}

// ---------------------------------------------------------------------------
// Navigation parsing
// ---------------------------------------------------------------------------
struct ParsedNavigation {
    tab_session_id: i32,
    index: i32,
    url: String,
    title: String,
}

fn parse_update_navigation(payload: &[u8]) -> Option<ParsedNavigation> {
    let mut r = PickleReader::from_pickle(payload);
    let tab_session_id = r.read_i32().ok()?;
    let index = r.read_i32().ok()?;
    let url = r.read_string().ok()?;
    let title = r.read_string16().unwrap_or_default();
    let _page_state = r.read_string().ok();
    let _transition = r.read_i32().ok();
    Some(ParsedNavigation {
        tab_session_id,
        index,
        url,
        title,
    })
}

// ---------------------------------------------------------------------------
// Session service files (Session_*): replay, but keep the closes.
// ---------------------------------------------------------------------------
#[derive(Default)]
struct SessionTabState {
    window_id: i32,
    index: i64,
    pinned: bool,
    navs: Vec<(i32, ParsedNavigation)>,
    current_nav_index: i64,
    group_token: Option<Vec<u8>>,
    last_active_ms: Option<i64>,
}

/// Picks the navigation the browser considers current for a tab.
///
/// Chromium clamps `current_navigation_index` to the navigation list after
/// pruning; without the clamps we do it by `index`: the selected navigation is
/// the one whose `index` matches, falling back to the highest index.
fn current_navigation<'a>(t: &'a SessionTabState) -> Option<&'a ParsedNavigation> {
    if t.navs.is_empty() {
        return None;
    }
    let mut by_index: Vec<&(i32, ParsedNavigation)> = t.navs.iter().collect();
    by_index.sort_by_key(|(i, _)| *i);
    if let Some(hit) = t
        .navs
        .iter()
        .map(|(_, n)| n)
        .find(|n| n.index as i64 == t.current_nav_index)
    {
        return Some(hit);
    }
    by_index.last().map(|(_, n)| n)
}

fn finalize_session_tab(t: SessionTabState, tab_session_id: i32) -> Option<RecoveredTab> {
    let nav = current_navigation(&t)?;
    Some(RecoveredTab {
        url: nav.url.clone(),
        title: nav.title.clone(),
        pinned: t.pinned,
        tab_session_id,
        window_id: t.window_id.to_string(),
        tab_index: t.index,
        group_token: t.group_token,
        group_name: None,
        group_color: None,
        closed_ms: None,
        last_active_ms: t.last_active_ms,
        confidence: Confidence::Medium,
    })
}

/// Borrow a tab entry, creating an empty default if absent. Free function
/// (not a closure) so it does not hold a mutable borrow of the map across
/// subsequent uses.
fn session_tab<'a>(
    tabs: &'a mut BTreeMap<i32, SessionTabState>,
    id: i32,
) -> &'a mut SessionTabState {
    tabs.entry(id).or_default()
}

fn parse_session_records(records: &[Record], out: &mut Recovery) {
    let mut tabs: BTreeMap<i32, SessionTabState> = BTreeMap::new();
    let mut windows_seen: BTreeMap<i32, ()> = BTreeMap::new();
    let mut groups: BTreeMap<Vec<u8>, RecoveredGroup> = BTreeMap::new();

    for rec in records {
        match rec.id {
            S_INITIAL_STATE_MARKER => {}
            S_SET_TAB_WINDOW => {
                match (read_i32_le(&rec.payload, 0), read_i32_le(&rec.payload, 4)) {
                    (Some(window_id), Some(tab_id)) => {
                        session_tab(&mut tabs, tab_id).window_id = window_id;
                        windows_seen.insert(window_id, ());
                    }
                    _ => note_skipped(out, rec),
                }
            }
            S_SET_TAB_INDEX_IN_WINDOW => {
                match (read_i32_le(&rec.payload, 0), read_i32_le(&rec.payload, 4)) {
                    (Some(tab_id), Some(index)) => session_tab(&mut tabs, tab_id).index = index as i64,
                    _ => note_skipped(out, rec),
                }
            }
            S_UPDATE_TAB_NAVIGATION => match parse_update_navigation(&rec.payload) {
                Some(nav) => {
                    let key = nav.index;
                    let t = session_tab(&mut tabs, nav.tab_session_id);
                    t.navs.push((key, nav));
                }
                None => note_skipped(out, rec),
            },
            S_SET_SELECTED_NAVIGATION_INDEX => {
                match (read_i32_le(&rec.payload, 0), read_i32_le(&rec.payload, 4)) {
                    (Some(tab_id), Some(index)) => {
                        session_tab(&mut tabs, tab_id).current_nav_index = index as i64
                    }
                    _ => note_skipped(out, rec),
                }
            }
            S_SET_SELECTED_TAB_IN_INDEX | S_SET_WINDOW_TYPE | S_SET_ACTIVE_WINDOW => {}
            S_SET_PINNED_STATE => {
                // {tab_id(4), pinned_state(bool, 1 byte)} — total 8 bytes on disk.
                match read_i32_le(&rec.payload, 0) {
                    Some(tab_id) if rec.payload.len() >= 5 => {
                        session_tab(&mut tabs, tab_id).pinned = rec.payload[4] != 0
                    }
                    _ => note_skipped(out, rec),
                }
            }
            S_SET_WINDOW_BOUNDS_3 => {
                // Bounds carry no recovery value; ignored by design.
            }
            S_LAST_ACTIVE_TIME => {
                if let (Some(tab_id), Some(ts)) = (
                    read_i32_le(&rec.payload, 0),
                    read_i64_le(&rec.payload, 4),
                ) {
                    session_tab(&mut tabs, tab_id).last_active_ms = windows_micros_to_unix_ms(ts);
                }
            }
            S_TAB_CLOSED => {
                // The browser would erase this tab; we keep it as a closed event.
                match (read_i32_le(&rec.payload, 0), read_i64_le(&rec.payload, 4)) {
                    (Some(tab_id), Some(close)) => {
                        if let Some(t) = tabs.remove(&tab_id) {
                            if let Some(mut rt) = finalize_session_tab(t, tab_id) {
                                rt.closed_ms = windows_micros_to_unix_ms(close);
                                out.closed_tabs.push(rt);
                            }
                        }
                    }
                    _ => note_skipped(out, rec),
                }
            }
            S_WINDOW_CLOSED => {
                match (read_i32_le(&rec.payload, 0), read_i64_le(&rec.payload, 4)) {
                    (Some(window_id), Some(close)) => {
                        windows_seen.remove(&window_id);
                        let closed_ms = windows_micros_to_unix_ms(close);
                        let mut recovered = RecoveredWindow {
                            window_id: window_id.to_string(),
                            closed_ms,
                            tabs: Vec::new(),
                            closing: true,
                        };
                        // Detach this window's tabs; they are part of the close.
                        let keys: Vec<i32> = tabs
                            .iter()
                            .filter(|(_, t)| t.window_id == window_id)
                            .map(|(k, _)| *k)
                            .collect();
                        for k in keys {
                            if let Some(t) = tabs.remove(&k) {
                                if let Some(mut rt) = finalize_session_tab(t, k) {
                                    rt.closed_ms = closed_ms;
                                    rt.window_id = window_id.to_string();
                                    recovered.tabs.push(rt);
                                }
                            }
                        }
                        recovered.tabs.sort_by_key(|t| t.tab_index);
                        out.windows.push(recovered);
                    }
                    _ => note_skipped(out, rec),
                }
            }
            S_NAVIGATION_PATH_PRUNED => {
                // {id, index, count}: drop navigations [index, index+count).
                match (
                    read_i32_le(&rec.payload, 0),
                    read_i32_le(&rec.payload, 4),
                    read_i32_le(&rec.payload, 8),
                ) {
                    (Some(tab_id), Some(index), Some(count)) if count > 0 => {
                        if let Some(t) = tabs.get_mut(&tab_id) {
                            let end = index.saturating_add(count);
                            t.navs.retain(|(i, _)| !(*i >= index && *i < end));
                        }
                    }
                    _ => note_skipped(out, rec),
                }
            }
            S_SET_TAB_GROUP => {
                // {tab_id(4), token(16), has_group(4)}
                match (read_i32_le(&rec.payload, 0), read_token16(&rec.payload, 4)) {
                    (Some(tab_id), Some(token)) => {
                        let token = if token.iter().all(|b| *b == 0) {
                            None
                        } else {
                            Some(token)
                        };
                        session_tab(&mut tabs, tab_id).group_token = token;
                    }
                    _ => note_skipped(out, rec),
                }
            }
            S_SET_TAB_GROUP_METADATA_2 => {
                // Pickle: token(16), title(string16), color(u32), collapsed?
                let token = rec.payload.get(0..16).map(|s| s.to_vec());
                if let Some(token) = token {
                    let body_offset = 16;
                    let mut r = PickleReader::new(&rec.payload[body_offset..]);
                    let title = r.read_string16().unwrap_or_default();
                    let color = r.read_u32().unwrap_or(0) as i64;
                    let collapsed = r.read_bool().unwrap_or(false);
                    groups.insert(
                        token.clone(),
                        RecoveredGroup {
                            token,
                            title,
                            color,
                            collapsed,
                        },
                    );
                } else {
                    note_skipped(out, rec);
                }
            }
            _ => note_skipped(out, rec),
        }
    }

    out.groups = groups.into_values().collect();

    // Everything still in `tabs` is the live state as of the last file write.
    let mut by_window: BTreeMap<i32, Vec<RecoveredTab>> = BTreeMap::new();
    for (id, t) in tabs {
        if let Some(rt) = finalize_session_tab(t, id) {
            by_window.entry(rt.window_id.parse().unwrap_or(0)).or_default().push(rt);
        }
    }
    for (window_id, mut tabs_vec) in by_window {
        tabs_vec.sort_by_key(|t| t.tab_index);
        out.windows.push(RecoveredWindow {
            window_id: window_id.to_string(),
            closed_ms: None,
            tabs: tabs_vec,
            closing: false,
        });
    }
    // Live window records that had no surviving tabs vanish from the windows list
    // but that is fine: they carried no tabs to restore.
}

// ---------------------------------------------------------------------------
// Tab restore files (Tabs_*): recently closed entries, grouped per window.
// ---------------------------------------------------------------------------
#[derive(Default)]
struct TabRestoreTabState {
    tab_session_id: i32,
    closed_ms: Option<i64>,
    selected_nav_index: i64,
    pinned: bool,
    navs: Vec<ParsedNavigation>,
    group_token: Option<Vec<u8>>,
}

#[derive(Default)]
struct TabRestoreWindowState {
    window_id: String,
    num_tabs: i32,
    closed_ms: Option<i64>,
    tabs: Vec<RecoveredTab>,
}

fn parse_tab_restore_records(records: &[Record], out: &mut Recovery) {
    let mut pending_window: Option<TabRestoreWindowState> = None;
    let mut current_tab: Option<TabRestoreTabState> = None;
    let mut groups: BTreeMap<Vec<u8>, RecoveredGroup> = BTreeMap::new();

    let flush_current_tab = |current_tab: &mut Option<TabRestoreTabState>,
                             pending_window: &mut Option<TabRestoreWindowState>,
                             out: &mut Recovery| {
        if let Some(tab) = current_tab.take() {
            // The persisted selected index refers to the *filtered* navigation
            // list; clamp to what we actually kept.
            let sel = tab.selected_nav_index.max(0) as usize;
            let pick = if tab.navs.is_empty() {
                None
            } else if sel < tab.navs.len() {
                Some(&tab.navs[sel])
            } else {
                tab.navs.last()
            };
            if let Some(nav) = pick {
                let mut rt = RecoveredTab {
                    url: nav.url.clone(),
                    title: nav.title.clone(),
                    pinned: tab.pinned,
                    tab_session_id: tab.tab_session_id,
                    window_id: pending_window
                        .as_ref()
                        .map(|w| w.window_id.clone())
                        .unwrap_or_else(|| "0".to_string()),
                    tab_index: 0, // assigned on flush
                    group_token: tab.group_token.clone(),
                    group_name: None,
                    group_color: None,
                    closed_ms: tab.closed_ms,
                    last_active_ms: None,
                    confidence: Confidence::High,
                };
                if let Some(w) = pending_window.as_mut() {
                    rt.tab_index = w.tabs.len() as i64;
                    w.tabs.push(rt);
                    if w.num_tabs > 0 && w.tabs.len() as i32 >= w.num_tabs {
                        out.windows.push(RecoveredWindow {
                            window_id: w.window_id.clone(),
                            closed_ms: w.closed_ms,
                            tabs: std::mem::take(&mut w.tabs),
                            closing: true,
                        });
                        *pending_window = None;
                    }
                } else {
                    out.closed_tabs.push(rt);
                }
            }
        }
    };

    let finalize_window = |pending_window: &mut Option<TabRestoreWindowState>,
                           out: &mut Recovery| {
        if let Some(w) = pending_window.take() {
            if !w.tabs.is_empty() {
                out.windows.push(RecoveredWindow {
                    window_id: w.window_id,
                    closed_ms: w.closed_ms,
                    tabs: w.tabs,
                    closing: true,
                });
            }
        }
    };

    for rec in records {
        match rec.id {
            T_UPDATE_TAB_NAVIGATION => match parse_update_navigation(&rec.payload) {
                Some(nav) => {
                    if let Some(tab) = current_tab.as_mut() {
                        tab.navs.push(nav);
                    } else {
                        // A navigation without a SelectedNavigationInTab marker:
                        // keep it as an anonymous tab rather than dropping data.
                        current_tab = Some(TabRestoreTabState {
                            tab_session_id: nav.tab_session_id,
                            closed_ms: None,
                            selected_nav_index: 0,
                            pinned: false,
                            navs: vec![nav],
                            group_token: None,
                        });
                    }
                }
                None => note_skipped(out, rec),
            },
            T_SELECTED_NAV_IN_TAB => {
                flush_current_tab(&mut current_tab, &mut pending_window, out);
                // Try the modern 16-byte payload, then the 8-byte legacy one.
                if let (Some(id), Some(index), ts) = (
                    read_i32_le(&rec.payload, 0),
                    read_i32_le(&rec.payload, 4),
                    read_i64_le(&rec.payload, 8),
                ) {
                    current_tab = Some(TabRestoreTabState {
                        tab_session_id: id,
                        closed_ms: ts.and_then(windows_micros_to_unix_ms),
                        selected_nav_index: index as i64,
                        pinned: false,
                        navs: Vec::new(),
                        group_token: None,
                    });
                } else {
                    note_skipped(out, rec);
                }
            }
            T_PINNED_STATE => {
                // Tab restore files store a bare 1-byte bool.
                if let Some(tab) = current_tab.as_mut() {
                    if rec.payload.first() == Some(&1) {
                        tab.pinned = true;
                    }
                } else {
                    note_skipped(out, rec);
                }
            }
            T_WINDOW | T_WINDOW_DEPRECATED => {
                // Flush whatever was open, close out any waiting window, then
                // open a new window context.
                flush_current_tab(&mut current_tab, &mut pending_window, out);
                finalize_window(&mut pending_window, out);
                match parse_tab_restore_window(&rec.payload, rec.id == T_WINDOW_DEPRECATED) {
                    Some(w) => {
                        pending_window = Some(w);
                    }
                    None => note_skipped(out, rec),
                }
            }
            T_CREATE_GROUP => {
                // Flush tabs so the flush can resolve group info, then parse.
                flush_current_tab(&mut current_tab, &mut pending_window, out);
                match parse_tab_restore_group(&rec.payload) {
                    Some(g) => {
                        groups.insert(g.token.clone(), g);
                    }
                    None => note_skipped(out, rec),
                }
            }
            T_TAB_GROUP_DATA => {
                // Attribute the group to the current tab. The group metadata
                // itself may arrive via T_CREATE_GROUP instead.
                if let Some(tab) = current_tab.as_mut() {
                    if let Some(token) = rec.payload.get(..16).map(|s| s.to_vec()) {
                        tab.group_token = Some(token.clone());
                    }
                } else {
                    note_skipped(out, rec);
                }
            }
            T_ADD_TAB_EXTRA_DATA
            | T_CREATE_SPLIT
            | T_SET_SPLIT_DATA
            | T_SET_EXTENSION_APP_ID
            | T_RESTORED_ENTRY
            | T_SET_WINDOW_APP_NAME
            | T_SET_WINDOW_USER_TITLE => {
                // Not needed for tab recovery.
            }
            _ => note_skipped(out, rec),
        }
    }

    // Flush whatever is still pending, in file order.
    flush_current_tab(&mut current_tab, &mut pending_window, out);
    finalize_window(&mut pending_window, out);

    out.groups = groups.into_values().collect();
}

fn parse_tab_restore_window(payload: &[u8], deprecated: bool) -> Option<TabRestoreWindowState> {
    let mut r = PickleReader::from_pickle(payload);
    let window_id = r.read_i32().ok()?;
    let selected_tab_index = r.read_i32().ok()?;
    let num_tabs = r.read_i32().ok()?;
    let timestamp = r.read_i64().ok()?;
    // Older layouts end after `timestamp`; newer ones carry bounds, type and
    // workspace. We only need the window identity and close time.
    let _ = (selected_tab_index,);
    let _ = deprecated;
    Some(TabRestoreWindowState {
        window_id: window_id.to_string(),
        num_tabs: num_tabs.max(0),
        closed_ms: windows_micros_to_unix_ms(timestamp),
        tabs: Vec::new(),
    })
}

fn parse_tab_restore_group(payload: &[u8]) -> Option<RecoveredGroup> {
    let token = payload.get(..16)?.to_vec();
    let mut r = PickleReader::new(&payload[16..]);
    let _session_id = r.read_i32().ok()?;
    let _num_tabs = r.read_i32().ok()?;
    let _browser_id = r.read_i32().ok()?;
    let title = r.read_string16().unwrap_or_default();
    let color = r.read_u32().unwrap_or(0) as i64;
    // Optional collapsed/saved fields: best effort.
    let is_collapsed = r.read_bool().unwrap_or(false);
    Some(RecoveredGroup {
        token,
        title,
        color,
        collapsed: is_collapsed,
    })
}
