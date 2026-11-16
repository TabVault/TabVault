//! Firefox session store (`sessionstore-backups/*.jsonlz4`) and history
//! (`places.sqlite`) parsers.
//!
//! **mozlz4** is Mozilla's variant of LZ4: an 8-byte ASCII magic `mozLz40\0`
//! followed by a **4-byte little-endian uncompressed length**, then the raw
//! LZ4 *block* (not the LZ4 frame format — the frame decompressor will fail).
//! This module decompresses with `lz4_flex`'s block codec.
//!
//! The decompressed payload is JSON:
//!
//! ```json
//! {
//!   "windows":   [ { "tabs": [ { "entries": [...], "index": N,
//!                              "lastAccessed": ms, "pinned": bool,
//!                              "userContextId": N, "selected": bool } ],
//!                    "_closedTabs": [ { "tab": {...}, "time": ms } ],
//!                    "selected": N } ],
//!   "_closedWindows": [ { "tabs": [...], "time": ms } ],
//!   "session": { "recentCrashes": N, ... }
//! }
//! ```
//!
//! All timestamps are **milliseconds since the Unix epoch**.
//!
//! The parser is deliberately tolerant: Firefox's schema evolves quietly, so
//! every field we read has a default and every unknown record is counted and
//! reported instead of failing the import.

use crate::error::{Error, Result};
use crate::model::{Confidence, Source, TabRecord};
use serde::Deserialize;
use std::collections::BTreeMap;

const MOZLZ4_MAGIC: &[u8; 8] = b"mozLz40\0";
/// Refuse to decompress absurd inputs — a corrupt length would otherwise try to
/// allocate gigabytes.
const MAX_DECOMPRESSED_BYTES: usize = 256 * 1024 * 1024;
const MAX_COMPRESSED_BYTES: usize = 64 * 1024 * 1024;

/// One recovered Firefox tab.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FirefoxTab {
    pub url: String,
    pub title: String,
    pub favicon: Option<String>,
    pub pinned: bool,
    pub container: Option<u32>,
    pub tab_index: i64,
    pub window_id: String,
    pub active_ms: i64,
    pub closed_ms: Option<i64>,
    /// Navigation entries in the tab's back-forward list (for diagnostics).
    pub nav_entries: usize,
    pub confidence: Confidence,
}

impl FirefoxTab {
    pub fn to_tab_record(&self, profile: &str, part_of_closed_window: bool) -> TabRecord {
        TabRecord {
            url: self.url.clone(),
            title: self.title.clone(),
            favicon: self.favicon.clone(),
            pinned: self.pinned,
            tab_index: self.tab_index,
            window_id: self.window_id.clone(),
            group_name: None,
            group_color: None,
            browser: "firefox".to_string(),
            profile: profile.to_string(),
            opened_ms: None,
            closed_ms: self.closed_ms,
            last_active_ms: None,
            active_ms: self.active_ms,
            visit_count: 0,
            part_of_closed_window,
            incognito: false,
            source: Source::FirefoxSessionStore,
            confidence: self.confidence,
            browser_tab_id: None,
        }
    }
}

/// A recovered Firefox window.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FirefoxWindow {
    pub window_id: String,
    pub closed_ms: Option<i64>,
    pub tabs: Vec<FirefoxTab>,
    pub closing: bool,
}

/// Recovery outcome for one session file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FirefoxRecovery {
    pub windows: Vec<FirefoxWindow>,
    /// Closed tabs from a closed window's `_closedTabs` belong to that window;
    /// these are top-level closed tabs seen in an open window's `_closedTabs`.
    pub closed_tabs: Vec<FirefoxTab>,
    pub closed_windows: usize,
    pub warnings: Vec<String>,
    /// Fields we could not understand (schema drift indicator).
    pub skipped: BTreeMap<String, usize>,
}

/// Decompresses a mozlz4 file.
pub fn mozlz4_decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < 12 {
        return Err(Error::BadFormat("mozlz4 file is shorter than its header".into()));
    }
    if &bytes[..8] != MOZLZ4_MAGIC {
        return Err(Error::BadFormat(
            "missing mozlz4 magic (expected 'mozLz40\\0')".into(),
        ));
    }
    let size = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if size == 0 {
        return Err(Error::BadFormat("mozlz4 declares a zero-length payload".into()));
    }
    if size > MAX_DECOMPRESSED_BYTES {
        return Err(Error::BadFormat(format!(
            "mozlz4 declares {size} bytes of output; refusing to allocate (cap {})",
            MAX_DECOMPRESSED_BYTES
        )));
    }
    let block = &bytes[12..];
    if block.len() > MAX_COMPRESSED_BYTES {
        return Err(Error::BadFormat(format!(
            "mozlz4 payload is {} bytes; refusing to process",
            block.len()
        )));
    }
    lz4_flex::block::decompress(block, size)
        .map_err(|e| Error::BadFormat(format!("lz4 block decompression failed: {e}")))
}

#[derive(Debug, Deserialize, Default)]
struct SessionStore {
    #[serde(default)]
    windows: Vec<SessionWindow>,
    #[serde(default, rename = "_closedWindows")]
    closed_windows: Vec<SessionClosedWindow>,
}

#[derive(Debug, Deserialize, Default)]
struct SessionClosedWindow {
    #[serde(default)]
    tabs: Vec<SessionTab>,
    /// Milliseconds since epoch when the window closed.
    #[serde(default)]
    time: Option<u64>,
}

#[derive(Debug, Deserialize, Default)]
struct SessionWindow {
    #[serde(default)]
    tabs: Vec<SessionTab>,
    /// Subset of windows this file records as closed.
    #[serde(default, rename = "_closedTabs")]
    closed_tabs: Vec<SessionClosedTab>,
    /// Firefox tracks the selected tab index; we do not need it for recovery.
    #[serde(default, rename = "selected")]
    #[allow(dead_code)]
    selected: i64,
}

#[derive(Debug, Deserialize, Default)]
struct SessionClosedTab {
    #[serde(default)]
    tab: Option<SessionTab>,
    #[serde(default)]
    time: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
struct SessionTab {
    #[serde(default)]
    entries: Vec<SessionEntry>,
    /// 1-based index into `entries` (the currently selected entry).
    #[serde(default = "default_index")]
    index: i64,
    #[serde(default, rename = "lastAccessed")]
    last_accessed: Option<i64>,
    #[serde(default)]
    pinned: bool,
    #[serde(default, rename = "hidden")]
    #[allow(dead_code)]
    hidden: bool,
    #[serde(default, rename = "userContextId")]
    user_context_id: Option<u32>,
    #[serde(default)]
    image: Option<String>,
}

fn default_index() -> i64 {
    1
}

#[derive(Debug, Deserialize, Default, Clone)]
struct SessionEntry {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    image: Option<String>,
}

/// Picks the navigation entry Firefox treats as current for a tab.
fn current_entry(tab: &SessionTab) -> Option<&SessionEntry> {
    if tab.entries.is_empty() {
        return None;
    }
    let idx = (tab.index.max(1) - 1) as usize;
    tab.entries.get(idx).or_else(|| tab.entries.last())
}

/// URLs worth reopening; about: pages and reader view wrappers add no value.
fn useful_url(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() {
        return false;
    }
    let lower = u.to_ascii_lowercase();
    !matches!(lower.as_str(), "about:blank" | "about:newtab" | "about:home" | "about:privatebrowsing" | "about:sessionrestore")
}

fn tab_from_session(tab: &SessionTab, window_id: &str, index: usize, closed_ms: Option<i64>) -> Option<FirefoxTab> {
    let entry = current_entry(tab)?;
    if !useful_url(&entry.url) {
        return None;
    }
    let summary = if entry.title.is_empty() {
        entry.url.clone()
    } else {
        entry.title.clone()
    };
    Some(FirefoxTab {
        url: entry.url.clone(),
        title: summary,
        favicon: tab.image.clone().or_else(|| entry.image.clone()),
        pinned: tab.pinned,
        container: tab.user_context_id.filter(|&id| id != 0),
        tab_index: index as i64,
        window_id: window_id.to_string(),
        active_ms: tab.last_accessed.unwrap_or(0),
        closed_ms,
        nav_entries: tab.entries.len(),
        confidence: Confidence::High,
    })
}

/// Parses a Firefox sessionstore file. `source_path` is used only for the
/// human-readable warnings.
pub fn parse_session_store(bytes: &[u8], source_path: &str) -> Result<FirefoxRecovery> {
    let json = mozlz4_decompress(bytes)?;
    let store: SessionStore = serde_json::from_slice(&json).map_err(|e| {
        Error::BadFormat(format!("decompressed session JSON at {source_path} does not parse: {e}"))
    })?;

    let mut out = FirefoxRecovery::default();

    // 1) Open windows — the live state as of the file's last write.
    for (wi, window) in store.windows.iter().enumerate() {
        let window_id = format!("firefox-{wi}");
        let mut fw = FirefoxWindow {
            window_id: window_id.clone(),
            closed_ms: None,
            tabs: Vec::new(),
            closing: false,
        };
        for (ti, tab) in window.tabs.iter().enumerate() {
            match tab_from_session(tab, &window_id, ti, None) {
                Some(t) => fw.tabs.push(t),
                None => {
                    *out.skipped.entry("empty_or_useless_entry".into()).or_insert(0) += 1;
                }
            }
        }
        if !fw.tabs.is_empty() {
            out.windows.push(fw);
        }
        // 2) Closed tabs recorded inside an open window.
        for ct in &window.closed_tabs {
            if let (Some(tab), time) = (ct.tab.as_ref(), ct.time) {
                match tab_from_session(tab, &format!("firefox-closed-in-{wi}"), 0, time.map(|t| t.max(0)))
                {
                    Some(t) => out.closed_tabs.push(t),
                    None => {
                        *out.skipped.entry("closed_tab_unparseable".into()).or_insert(0) += 1;
                    }
                }
            }
        }
    }

    // 3) Closed windows — the highest-value recovery data in the file.
    for (wi, closed) in store.closed_windows.iter().enumerate() {
        let closed_ms = closed.time.map(|t| t as i64);
        let window_id = format!("firefox-closed-{wi}");
        let mut fw = FirefoxWindow {
            window_id: window_id.clone(),
            closed_ms,
            tabs: Vec::new(),
            closing: true,
        };
        for (ti, tab) in closed.tabs.iter().enumerate() {
            match tab_from_session(tab, &window_id, ti, closed_ms) {
                Some(t) => fw.tabs.push(t),
                None => {
                    *out.skipped.entry("closed_window_tab_unparseable".into()).or_insert(0) += 1;
                }
            }
        }
        if !fw.tabs.is_empty() {
            out.closed_windows += 1;
            out.windows.push(fw);
        }
    }

    Ok(out)
}

/// Which fields each close action carries — documented for callers.
pub const fn closed_window_marker() -> &'static str {
    "_closedWindows"
}

// ---------------------------------------------------------------------------
// places.sqlite
// ---------------------------------------------------------------------------

/// One history row from `places.sqlite`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacesRow {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    /// Microseconds since the Unix epoch, exactly as Firefox stores it.
    pub last_visit_date: Option<i64>,
    pub first_visit_date: Option<i64>,
}

impl PlacesRow {
    pub fn last_visit_ms(&self) -> Option<i64> {
        // Firefox stores **microseconds** in moz_places.
        self.last_visit_date.map(|us| us / 1000)
    }

    pub fn first_visit_ms(&self) -> Option<i64> {
        self.first_visit_date.map(|us| us / 1000)
    }
}

/// Reads the most useful slice of `places.sqlite`.
///
/// `db` must be opened on a *copy*: the live database is constantly written by
/// the browser and cannot be opened reliably. The caller performs the copy.
pub fn read_places(conn: &rusqlite::Connection) -> Result<Vec<PlacesRow>> {
    let mut stmt = conn.prepare(
        "SELECT url, COALESCE(title, ''), visit_count, last_visit_date, first_visit_date
         FROM moz_places
         WHERE visit_count > 0 AND url NOT LIKE 'place:%'
         ORDER BY last_visit_date DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(PlacesRow {
            url: r.get(0)?,
            title: r.get(1)?,
            visit_count: r.get(2)?,
            last_visit_date: r.get(3)?,
            first_visit_date: r.get(4)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}
