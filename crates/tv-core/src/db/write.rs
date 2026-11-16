//! Write-side database operations.
//!
//! The native host "ingests" a full browser state. To keep the extension dumb
//! (and this logic unit-testable), all decision-making lives here in Rust:
//! fingerprint dedupe, closed-tab detection, recovery points and pinned-vault
//! bookkeeping.
//!
//! A "state" is one browser profile's complete picture at one instant. The
//! stored `live_state` row lets the host survive its own restart: the previous
//! state is read from the database, not from any process's memory.
//!
//! History note: `closed_window_events` and `closed_tab_events` are PERMANENT —
//! retention never deletes them.

use crate::db::{url_key, url_words, Db};
use crate::error::Result;
use crate::model::{
    BrowserSnapshot, Confidence, SnapshotMeta, SnapshotReason, Source, TabRecord,
};
use crate::retention;
use crate::snapshot;
use crate::time::now_ms;
use crate::url_util;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// What happened as a result of an ingest call.
#[derive(Debug, Clone, PartialEq)]
pub enum IngestOutcome {
    /// Settings pause recording; nothing was written.
    Paused,
    /// State identical to the stored one; no new snapshot row.
    Unchanged { snapshot_id: i64 },
    /// A new snapshot was stored.
    Stored {
        snapshot_id: i64,
        fingerprint: String,
        tabs_removed: usize,
        recovery_point: bool,
    },
}

/// Payload for a closed-tab event derived from a removed tab.
#[derive(Debug, Clone)]
pub struct ClosedTabRecord {
    pub browser: String,
    pub profile: String,
    pub url: String,
    pub title: String,
    pub favicon: Option<String>,
    pub pinned: bool,
    pub tab_index: i64,
    pub window_id: String,
    pub group_name: Option<String>,
    pub group_color: Option<String>,
    pub opened_ms: Option<i64>,
    pub closed_ms: i64,
    pub last_active_ms: Option<i64>,
    pub active_ms: i64,
    pub visit_count: i64,
    pub part_of_closed_window: bool,
    pub source: Source,
    pub confidence: Confidence,
}

impl ClosedTabRecord {
    /// Normalised key for de-duplication and grouping by page.
    pub fn key(&self) -> String {
        url_key(&self.url)
    }
}

fn tab_to_closed_record(
    tab: &TabRecord,
    browser: &str,
    profile: &str,
    closed_ms: i64,
    part_of_closed_window: bool,
) -> ClosedTabRecord {
    ClosedTabRecord {
        browser: browser.to_string(),
        profile: profile.to_string(),
        url: tab.url.clone(),
        title: tab.title.clone(),
        favicon: tab.favicon.clone(),
        pinned: tab.pinned,
        tab_index: tab.tab_index,
        window_id: tab.window_id.clone(),
        group_name: tab.group_name.clone(),
        group_color: tab.group_color.clone(),
        opened_ms: tab.opened_ms,
        closed_ms,
        last_active_ms: tab.last_active_ms,
        active_ms: tab.active_ms,
        visit_count: tab.visit_count,
        part_of_closed_window,
        source: tab.source,
        confidence: tab.confidence,
    }
}

// ---------------------------------------------------------------------------
// Search index
// ---------------------------------------------------------------------------

/// Upserts one page into the deduplicated search index.
///
/// `doc_key = browser|profile|url_key`; the FTS mirror is maintained by the
/// triggers declared in migration 1.
#[allow(clippy::too_many_arguments)]
pub fn search_docs_upsert(
    conn: &Connection,
    browser: &str,
    profile: &str,
    url: &str,
    title: &str,
    kind: &str,
    pinned: bool,
    visit_count: i64,
    active_ms: i64,
    last_seen_ms: i64,
) -> Result<()> {
    let ukey = url_key(url);
    if ukey.is_empty() {
        return Ok(());
    }
    let domain = url_util::registrable_domain(url);
    let key = format!("{browser}|{profile}|{ukey}");
    conn.execute(
        "INSERT INTO search_docs (doc_key, browser, profile, url, url_key, title, domain, url_words, kind, pinned, visit_count, active_ms, last_seen_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(doc_key) DO UPDATE SET
            title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE search_docs.title END,
            url = CASE WHEN search_docs.url = '' THEN excluded.url ELSE search_docs.url END,
            pinned = MAX(search_docs.pinned, excluded.pinned),
            visit_count = search_docs.visit_count + excluded.visit_count,
            active_ms = search_docs.active_ms + excluded.active_ms,
            last_seen_ms = MAX(search_docs.last_seen_ms, excluded.last_seen_ms)",
        rusqlite::params![
            key,
            browser,
            profile,
            url,
            ukey,
            title,
            domain,
            url_words(url),
            kind,
            pinned as i64,
            visit_count,
            active_ms,
            last_seen_ms,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Visit statistics
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn upsert_visit_stats(
    conn: &Connection,
    browser: &str,
    profile: &str,
    url: &str,
    title: &str,
    favicon: Option<&str>,
    visit_increment: i64,
    active_increment_ms: i64,
    first_seen_ms: i64,
    last_seen_ms: i64,
    pinned: bool,
) -> Result<()> {
    let ukey = url_key(url);
    if ukey.is_empty() {
        return Ok(());
    }
    let domain = url_util::registrable_domain(url);
    conn.execute(
        "INSERT INTO visit_stats (browser, profile, domain, url, url_key, title, favicon, visit_count, active_ms, first_seen_ms, last_seen_ms, pinned)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(browser, profile, url_key) DO UPDATE SET
            title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE visit_stats.title END,
            favicon = COALESCE(excluded.favicon, visit_stats.favicon),
            domain = excluded.domain,
            visit_count = visit_stats.visit_count + excluded.visit_count,
            active_ms = visit_stats.active_ms + excluded.active_ms,
            last_seen_ms = MAX(visit_stats.last_seen_ms, excluded.last_seen_ms),
            pinned = MAX(visit_stats.pinned, excluded.pinned)",
        rusqlite::params![
            browser,
            profile,
            domain,
            url,
            ukey,
            title,
            favicon,
            visit_increment,
            active_increment_ms,
            first_seen_ms,
            last_seen_ms,
            pinned as i64,
        ],
    )?;
    Ok(())
}

/// Sets (rather than adds) visit counts from a historic source, e.g. a
/// browser's history database. Used by the history importer.
#[allow(clippy::too_many_arguments)]
pub fn seed_visit_stats(
    conn: &Connection,
    browser: &str,
    profile: &str,
    url: &str,
    title: &str,
    visit_count: i64,
    last_seen_ms: Option<i64>,
    first_seen_ms: Option<i64>,
) -> Result<()> {
    let ukey = url_key(url);
    if ukey.is_empty() || visit_count <= 0 {
        return Ok(());
    }
    let domain = url_util::registrable_domain(url);
    let last_seen = last_seen_ms.unwrap_or_else(now_ms);
    let first_seen = first_seen_ms.unwrap_or(last_seen);
    conn.execute(
        "INSERT INTO visit_stats (browser, profile, domain, url, url_key, title, favicon, visit_count, active_ms, first_seen_ms, last_seen_ms, pinned)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, 0, ?8, ?9, 0)
         ON CONFLICT(browser, profile, url_key) DO UPDATE SET
            title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE visit_stats.title END,
            visit_count = MAX(visit_stats.visit_count, excluded.visit_count),
            last_seen_ms = MAX(visit_stats.last_seen_ms, excluded.last_seen_ms),
            first_seen_ms = MIN(visit_stats.first_seen_ms, excluded.first_seen_ms)",
        rusqlite::params![
            browser,
            profile,
            domain,
            url,
            ukey,
            title,
            visit_count,
            first_seen,
            last_seen,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Pinned vault
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn touch_pinned_site(
    conn: &Connection,
    browser: &str,
    profile: &str,
    url: &str,
    title: &str,
    favicon: Option<&str>,
    pinned_now: bool,
    group_name: Option<&str>,
    at_ms: i64,
) -> Result<()> {
    let ukey = url_key(url);
    if ukey.is_empty() {
        return Ok(());
    }
    let domain = url_util::registrable_domain(url);
    conn.execute(
        "INSERT INTO pinned_sites (browser, profile, url, url_key, title, favicon, domain, first_pinned_ms, last_pinned_ms, pin_count, currently_pinned, group_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11)
         ON CONFLICT(browser, profile, url_key) DO UPDATE SET
            title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE pinned_sites.title END,
            favicon = COALESCE(excluded.favicon, pinned_sites.favicon),
            last_pinned_ms = MAX(pinned_sites.last_pinned_ms, excluded.last_pinned_ms),
            pin_count = pinned_sites.pin_count
                + CASE WHEN excluded.currently_pinned = 1 AND pinned_sites.currently_pinned = 0 THEN 1 ELSE 0 END,
            currently_pinned = excluded.currently_pinned,
            group_name = COALESCE(excluded.group_name, pinned_sites.group_name)",
        rusqlite::params![
            browser,
            profile,
            url,
            ukey,
            title,
            favicon,
            domain,
            at_ms,
            at_ms,
            pinned_now as i64,
            group_name,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Closed-tab / closed-window events
// ---------------------------------------------------------------------------

/// Inserts a closed-tab event and folds its statistics into
/// `visit_stats`/`search_docs`.
fn insert_closed_tab_event(
    conn: &Connection,
    ev: &ClosedTabRecord,
    window_event_id: Option<i64>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO closed_tab_events (
            browser, profile, url, url_key, title, favicon, pinned, tab_index, window_id,
            group_name, group_color, opened_ms, closed_ms, last_active_ms, active_ms,
            visit_count, part_of_closed_window, source, confidence, window_event_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        rusqlite::params![
            ev.browser,
            ev.profile,
            ev.url,
            ev.key(),
            ev.title,
            ev.favicon,
            ev.pinned as i64,
            ev.tab_index,
            ev.window_id,
            ev.group_name,
            ev.group_color,
            ev.opened_ms,
            ev.closed_ms,
            ev.last_active_ms,
            ev.active_ms,
            ev.visit_count,
            ev.part_of_closed_window as i64,
            ev.source.as_str(),
            ev.confidence.as_str(),
            window_event_id,
        ],
    )?;
    let id = conn.last_insert_rowid();
    upsert_visit_stats(
        conn,
        &ev.browser,
        &ev.profile,
        &ev.url,
        &ev.title,
        ev.favicon.as_deref(),
        1,
        ev.active_ms,
        ev.opened_ms.unwrap_or(ev.closed_ms),
        ev.closed_ms,
        ev.pinned,
    )?;
    search_docs_upsert(
        conn,
        &ev.browser,
        &ev.profile,
        &ev.url,
        &ev.title,
        if ev.pinned { "pinned" } else { "closed" },
        ev.pinned,
        1,
        ev.active_ms,
        ev.closed_ms,
    )?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
fn insert_window_event(
    conn: &Connection,
    browser: &str,
    profile: &str,
    window_id: &str,
    closed_ms: i64,
    tab_count: i64,
    pinned_count: i64,
    snapshot_id: Option<i64>,
    source: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO closed_window_events
            (browser, profile, window_id, closed_ms, tab_count, pinned_count, snapshot_id, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            browser,
            profile,
            window_id,
            closed_ms,
            tab_count,
            pinned_count,
            snapshot_id,
            source,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

// ---------------------------------------------------------------------------
// Full-state ingest (the host's main path)
// ---------------------------------------------------------------------------

/// The result of ingesting one live browser state.
#[derive(Debug)]
pub struct IngestReport {
    pub outcome: IngestOutcome,
    /// Closed tabs derived from the diff (live ingest only), in diff order.
    pub closed_tabs: Vec<ClosedTabRecord>,
    /// Closed windows derived from the diff (live ingest only).
    pub closed_windows: usize,
}

/// Ingests one browser profile's full state.
///
/// The order of operations matters: pause/exclusions first, then dedupe, then
/// storage, then close detection, then stats — so a re-ingested identical state
/// performs no work at all.
pub fn ingest_full_state(
    db: &Db,
    snap: &BrowserSnapshot,
    reason: SnapshotReason,
) -> Result<IngestReport> {
    let settings = db.settings();
    if settings.recording_paused {
        return Ok(IngestReport {
            outcome: IngestOutcome::Paused,
            closed_tabs: Vec::new(),
            closed_windows: 0,
        });
    }

    let (filtered, dropped) =
        snapshot::filter_snapshot(snap.clone(), &settings.exclusions, settings.capture_incognito);
    let fp = snapshot::fingerprint(&filtered);
    let now = now_ms();
    let browser = filtered.browser.clone();
    let profile = filtered.profile.clone();

    let conn = db.conn();

    // Previous live state, if any.
    let prev: Option<BrowserSnapshot> = conn
        .query_row(
            "SELECT payload FROM live_state WHERE browser = ?1 AND profile = ?2",
            rusqlite::params![browser, profile],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok());

    // Fast dedupe: identical state → no snapshot row.
    let existing_snapshot: Option<i64> = conn
        .query_row(
            "SELECT id FROM snapshots WHERE browser = ?1 AND profile = ?2 AND fingerprint = ?3",
            rusqlite::params![browser, profile, fp],
            |r| r.get(0),
        )
        .ok();
    if let Some(id) = existing_snapshot {
        let payload = serde_json::to_string(&filtered)?;
        conn.execute(
            "INSERT INTO live_state (browser, profile, fingerprint, payload, updated_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(browser, profile) DO UPDATE SET
                fingerprint = excluded.fingerprint,
                payload = excluded.payload,
                updated_ms = excluded.updated_ms",
            rusqlite::params![browser, profile, fp, payload, now],
        )?;
        for (_w, t) in filtered.all_tabs() {
            touch_pinned_site(
                conn,
                &browser,
                &profile,
                &t.url,
                &t.title,
                t.favicon.as_deref(),
                t.pinned,
                t.group_name.as_deref(),
                now,
            )?;
        }
        return Ok(IngestReport {
            outcome: IngestOutcome::Unchanged { snapshot_id: id },
            closed_tabs: Vec::new(),
            closed_windows: 0,
        });
    }

    let diff = match &prev {
        Some(before) => snapshot::diff(before, &filtered),
        None => snapshot::SnapshotDiff::default(),
    };

    let mut recovery_point = reason.is_recovery_point();
    // Diff-detected closes only for live capture — imported recovery data has
    // its own timestamps and must not synthesize "mass close" events.
    let detect_closes = !matches!(reason, SnapshotReason::Imported);

    let mut closed_tabs: Vec<ClosedTabRecord> = Vec::new();
    if detect_closes {
        for t in &diff.removed {
            closed_tabs.push(tab_to_closed_record(t, &browser, &profile, now, false));
        }
    }
    if detect_closes && closed_tabs.len() as u64 >= settings.mass_close_tab_threshold as u64 {
        recovery_point = true;
    }

    let tx = db.conn().unchecked_transaction()?;
    let snapshot_id = insert_snapshot_row(
        &tx,
        &browser,
        &profile,
        &fp,
        &filtered,
        reason,
        recovery_point,
        dropped as i64,
        now,
    )?;
    insert_window_and_tab_rows(&tx, snapshot_id, &browser, &profile, &filtered)?;

    let mut closed_windows = 0usize;
    if detect_closes {
        let before = prev.unwrap_or_default();
        // Closed windows first, then the tabs that went with them.
        for before_window in &before.windows {
            if diff
                .windows_removed
                .iter()
                .any(|id| id == &before_window.window_id)
            {
                let tab_count = before_window.tabs.len() as i64;
                let pinned = before_window.tabs.iter().filter(|t| t.pinned).count() as i64;
                let event_id = insert_window_event(
                    &tx,
                    &browser,
                    &profile,
                    &before_window.window_id,
                    now,
                    tab_count,
                    pinned,
                    Some(snapshot_id),
                    "extension",
                )?;
                closed_windows += 1;
                for t in &before_window.tabs {
                    let rec = tab_to_closed_record(t, &browser, &profile, now, true);
                    insert_closed_tab_event(&tx, &rec, Some(event_id))?;
                    closed_tabs.push(rec);
                }
            }
        }
        // Single-tab closes not attached to a window close.
        for rec in filtered_closes_only(&closed_tabs) {
            insert_closed_tab_event(&tx, rec, None)?;
        }
        if closed_windows >= 1 {
            recovery_point = true;
        }
    }

    // Pinned vault from the filtered (surviving) state.
    for (_w, t) in filtered.all_tabs() {
        touch_pinned_site(
            &tx,
            &browser,
            &profile,
            &t.url,
            &t.title,
            t.favicon.as_deref(),
            t.pinned,
            t.group_name.as_deref(),
            now,
        )?;
    }

    // Live state overwrite for the next diff.
    let payload = serde_json::to_string(&filtered)?;
    tx.execute(
        "INSERT INTO live_state (browser, profile, fingerprint, payload, updated_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(browser, profile) DO UPDATE SET
            fingerprint = excluded.fingerprint,
            payload = excluded.payload,
            updated_ms = excluded.updated_ms",
        rusqlite::params![browser, profile, fp, payload, now],
    )?;

    // Mass-close promotion from the recent-close window.
    if detect_closes {
        let window_ms = settings.mass_close_window_secs as i64 * 1000;
        let cutoff = now - window_ms;
        let recent: i64 = tx.query_row(
            "SELECT count(*) FROM closed_tab_events
             WHERE browser = ?1 AND profile = ?2 AND closed_ms >= ?3",
            rusqlite::params![browser, profile, cutoff],
            |r| r.get(0),
        )?;
        if recent as u64 >= settings.mass_close_tab_threshold as u64 {
            recovery_point = true;
        }
        if recovery_point {
            tx.execute(
                "UPDATE snapshots SET recovery_point = 1, reason = ?1 WHERE id = ?2",
                rusqlite::params![SnapshotReason::MassClose.as_str(), snapshot_id],
            )?;
        }
    }

    tx.commit()?;

    Ok(IngestReport {
        outcome: IngestOutcome::Stored {
            snapshot_id,
            fingerprint: fp,
            tabs_removed: closed_tabs.len(),
            recovery_point,
        },
        closed_tabs,
        closed_windows,
    })
}

/// The `part_of_closed_window` duplication would otherwise double-count these
/// rows; this selects only the standalone ones.
fn filtered_closes_only(closed_tabs: &[ClosedTabRecord]) -> impl Iterator<Item = &ClosedTabRecord> {
    closed_tabs.iter().filter(|t| !t.part_of_closed_window)
}

fn insert_snapshot_row(
    conn: &Connection,
    browser: &str,
    profile: &str,
    fingerprint: &str,
    snap: &BrowserSnapshot,
    reason: SnapshotReason,
    recovery_point: bool,
    dropped: i64,
    now_ms: i64,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO snapshots (taken_ms, browser, profile, fingerprint, tab_count, window_count, reason, recovery_point, dropped_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            now_ms,
            browser,
            profile,
            fingerprint,
            snap.tab_count() as i64,
            snap.windows.len() as i64,
            reason.as_str(),
            recovery_point as i64,
            dropped,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn insert_window_and_tab_rows(
    conn: &Connection,
    snapshot_id: i64,
    browser: &str,
    profile: &str,
    snap: &BrowserSnapshot,
) -> Result<()> {
    for window in &snap.windows {
        conn.execute(
            "INSERT INTO windows (snapshot_id, window_id, focused, incognito, tab_count)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                snapshot_id,
                window.window_id,
                window.focused as i64,
                window.incognito as i64,
                window.tabs.len() as i64,
            ],
        )?;
        let window_row_id = conn.last_insert_rowid();
        for tab in &window.tabs {
            conn.execute(
                "INSERT INTO tabs (
                    snapshot_id, window_row_id, url, url_key, title, favicon, pinned, tab_index,
                    group_name, group_color, browser, profile, source, confidence, opened_ms,
                    closed_ms, last_active_ms, active_ms, visit_count, part_of_closed_window, incognito
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                rusqlite::params![
                    snapshot_id,
                    window_row_id,
                    tab.url,
                    url_key(&tab.url),
                    tab.title,
                    tab.favicon,
                    tab.pinned as i64,
                    tab.tab_index,
                    tab.group_name,
                    tab.group_color,
                    browser,
                    profile,
                    tab.source.as_str(),
                    tab.confidence.as_str(),
                    tab.opened_ms,
                    tab.closed_ms,
                    tab.last_active_ms,
                    tab.active_ms,
                    tab.visit_count,
                    tab.part_of_closed_window as i64,
                    window.incognito || tab.incognito,
                ],
            )?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Import batches
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ImportStats {
    pub items_found: usize,
    pub items_imported: usize,
}

/// Imports recovered tabs as closed-tab events, tagged by their source.
pub fn import_recovered_tabs(
    db: &Db,
    batch_id: i64,
    _source_label: &str,
    browser: &str,
    profile: &str,
    tabs: &[(TabRecord, Option<i64>)],
    part_of_closed_window: bool,
) -> Result<ImportStats> {
    let mut stats = ImportStats {
        items_found: tabs.len(),
        ..Default::default()
    };
    let now = now_ms();
    let tx = db.conn().unchecked_transaction()?;
    for (tab, closed_ms) in tabs {
        let rec = ClosedTabRecord {
            browser: browser.to_string(),
            profile: profile.to_string(),
            url: tab.url.clone(),
            title: tab.title.clone(),
            favicon: tab.favicon.clone(),
            pinned: tab.pinned,
            tab_index: tab.tab_index,
            window_id: tab.window_id.clone(),
            group_name: tab.group_name.clone(),
            group_color: tab.group_color.clone(),
            opened_ms: tab.opened_ms,
            closed_ms: closed_ms.unwrap_or(now),
            last_active_ms: tab.last_active_ms,
            active_ms: tab.active_ms,
            visit_count: tab.visit_count.max(1),
            part_of_closed_window,
            source: tab.source,
            confidence: tab.confidence,
        };
        insert_closed_tab_event(&tx, &rec, None)?;
        touch_pinned_site(
            &tx,
            browser,
            profile,
            &tab.url,
            &tab.title,
            tab.favicon.as_deref(),
            tab.pinned,
            tab.group_name.as_deref(),
            closed_ms.unwrap_or(now),
        )?;
        stats.items_imported += 1;
    }
    tx.execute(
        "UPDATE import_batches SET items_found = items_found + ?1, items_imported = items_imported + ?2 WHERE id = ?3",
        rusqlite::params![tabs.len() as i64, stats.items_imported as i64, batch_id],
    )?;
    tx.commit()?;
    Ok(stats)
}

/// Imports a recovered window as a closed_window_events row plus its tabs.
#[allow(clippy::too_many_arguments)]
pub fn import_recovered_window(
    db: &Db,
    batch_id: i64,
    browser: &str,
    profile: &str,
    window_id: &str,
    closed_ms: Option<i64>,
    tabs: &[TabRecord],
    source: &str,
) -> Result<i64> {
    let now = now_ms();
    let closed = closed_ms.unwrap_or(now);
    let tab_count = tabs.len() as i64;
    let pinned = tabs.iter().filter(|t| t.pinned).count() as i64;
    let tx = db.conn().unchecked_transaction()?;
    let event_id = insert_window_event(&tx, browser, profile, window_id, closed, tab_count, pinned, None, source)?;
    for tab in tabs {
        let rec = ClosedTabRecord {
            browser: browser.to_string(),
            profile: profile.to_string(),
            url: tab.url.clone(),
            title: tab.title.clone(),
            favicon: tab.favicon.clone(),
            pinned: tab.pinned,
            tab_index: tab.tab_index,
            window_id: tab.window_id.clone(),
            group_name: tab.group_name.clone(),
            group_color: tab.group_color.clone(),
            opened_ms: tab.opened_ms,
            closed_ms: closed,
            last_active_ms: tab.last_active_ms,
            active_ms: tab.active_ms,
            visit_count: tab.visit_count.max(1),
            part_of_closed_window: true,
            source: tab.source,
            confidence: tab.confidence,
        };
        insert_closed_tab_event(&tx, &rec, Some(event_id))?;
        touch_pinned_site(
            &tx,
            browser,
            profile,
            &tab.url,
            &tab.title,
            tab.favicon.as_deref(),
            tab.pinned,
            tab.group_name.as_deref(),
            closed,
        )?;
    }
    tx.execute(
        "UPDATE import_batches SET items_found = items_found + ?1, items_imported = items_imported + ?2 WHERE id = ?3",
        rusqlite::params![tabs.len() as i64, tabs.len() as i64, batch_id],
    )?;
    tx.commit()?;
    Ok(event_id)
}

/// Creates a fresh import batch row.
pub fn begin_import_batch(
    db: &Db,
    source: &str,
    source_path: Option<&Path>,
    browser: Option<&str>,
    profile: Option<&str>,
) -> Result<i64> {
    db.conn().execute(
        "INSERT INTO import_batches (started_ms, source, source_path, browser, profile, status)
         VALUES (?1, ?2, ?3, ?4, ?5, 'running')",
        rusqlite::params![
            now_ms(),
            source,
            source_path.map(|p| p.display().to_string()),
            browser,
            profile,
        ],
    )?;
    Ok(db.conn().last_insert_rowid())
}

pub fn finish_import_batch(db: &Db, batch_id: i64, error: Option<&str>) -> Result<()> {
    db.conn().execute(
        "UPDATE import_batches
         SET finished_ms = ?1, status = ?2, error = ?3
         WHERE id = ?4",
        rusqlite::params![
            now_ms(),
            if error.is_some() { "failed" } else { "done" },
            error,
            batch_id,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

/// Applies the configured retention policy to snapshots and low-value visit
/// stats. Closed-tab/window history is permanent and is never deleted here.
///
/// Returns removed-row counts for the diagnostics screen.
pub fn apply_retention(db: &Db, now: i64) -> Result<BTreeMap<String, usize>> {
    let settings = db.settings();
    let policy = retention::RetentionPolicy {
        keep_all_hours: settings.retention_keep_all_hours,
        hourly_days: settings.retention_hourly_days,
        daily_days: settings.retention_daily_days,
        keep_recovery_points_forever: settings.keep_recovery_points_forever,
    };

    let mut metas: Vec<SnapshotMeta> = {
        let mut stmt = db.conn().prepare(
            "SELECT id, taken_ms, browser, profile, fingerprint, tab_count, window_count, reason, recovery_point
             FROM snapshots",
        )?;
        let collected = stmt
            .query_map([], |r| {
                Ok(SnapshotMeta {
                    id: r.get(0)?,
                    taken_ms: r.get(1)?,
                    browser: r.get(2)?,
                    profile: r.get(3)?,
                    fingerprint: r.get(4)?,
                    tab_count: r.get(5)?,
                    window_count: r.get(6)?,
                    reason: SnapshotReason::parse(&r.get::<_, String>(7)?).unwrap_or(SnapshotReason::Interval),
                    recovery_point: r.get::<_, i64>(8)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        collected
    };
    metas.sort_by_key(|m| m.taken_ms);

    let plan = retention::plan(&metas, policy, now);
    let mut removed = BTreeMap::new();

    if !plan.delete.is_empty() {
        let tx = db.conn().unchecked_transaction()?;
        for chunk in plan.delete.chunks(500) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM snapshots WHERE id IN ({placeholders})");
            tx.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
        }
        tx.commit()?;
        removed.insert("snapshots".to_string(), plan.delete.len());
    }

    // Visit stats: keep anything with real usage.
    let doomed: Vec<i64> = {
        let mut stmt = db.conn().prepare(
            "SELECT id, visit_count, active_ms, last_seen_ms FROM visit_stats",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        let collected = rows
            .filter_map(|r| r.ok())
            .filter(|(_, vc, active, last_seen)| {
                !retention::visit_stat_is_worth_keeping(*vc, *active, *last_seen, now)
            })
            .map(|(id, _, _, _)| id)
            .collect();
        collected
    };
    if !doomed.is_empty() {
        let tx = db.conn().unchecked_transaction()?;
        for chunk in doomed.chunks(500) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM visit_stats WHERE id IN ({placeholders})");
            tx.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
        }
        tx.commit()?;
        removed.insert("visit_stats".to_string(), doomed.len());
    }

    Ok(removed)
}

/// Computes the retention plan without deleting anything.
pub fn preview_retention(db: &Db, now: i64) -> Result<(usize, usize, usize, usize)> {
    let settings = db.settings();
    let policy = retention::RetentionPolicy {
        keep_all_hours: settings.retention_keep_all_hours,
        hourly_days: settings.retention_hourly_days,
        daily_days: settings.retention_daily_days,
        keep_recovery_points_forever: settings.keep_recovery_points_forever,
    };
    let mut metas: Vec<SnapshotMeta> = {
        let mut stmt = db.conn().prepare(
            "SELECT id, taken_ms, browser, profile, fingerprint, tab_count, window_count, reason, recovery_point FROM snapshots",
        )?;
        let collected = stmt
            .query_map([], |r| {
                Ok(SnapshotMeta {
                    id: r.get(0)?,
                    taken_ms: r.get(1)?,
                    browser: r.get(2)?,
                    profile: r.get(3)?,
                    fingerprint: r.get(4)?,
                    tab_count: r.get(5)?,
                    window_count: r.get(6)?,
                    reason: SnapshotReason::parse(&r.get::<_, String>(7)?).unwrap_or(SnapshotReason::Interval),
                    recovery_point: r.get::<_, i64>(8)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        collected
    };
    metas.sort_by_key(|m| m.taken_ms);
    let plan = retention::plan(&metas, policy, now);
    Ok((plan.kept_recent, plan.kept_hourly, plan.kept_daily, plan.delete.len()))
}

// ---------------------------------------------------------------------------
// Restore request queue (desktop app -> native host -> browser extension)
// ---------------------------------------------------------------------------

/// Enqueues a restore job for the extension to pick up.
///
/// The payload is opaque here — the extension interprets it — so the desktop
/// app and the host can never disagree about its shape.
pub fn enqueue_restore_request(
    db: &Db,
    action: &str,
    payload: &serde_json::Value,
) -> Result<i64> {
    db.conn().execute(
        "INSERT INTO restore_requests (created_ms, action, payload, status)
         VALUES (?1, ?2, ?3, 'pending')",
        rusqlite::params![now_ms(), action, payload.to_string()],
    )?;
    Ok(db.conn().last_insert_rowid())
}

/// Lists pending restore jobs (used by the host when the extension polls).
pub fn drain_restore_requests(db: &Db, limit: u32) -> Result<Vec<(i64, String, String)>> {
    let mut stmt = db.conn().prepare(
        "SELECT id, action, payload FROM restore_requests
         WHERE status = 'pending' ORDER BY id ASC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !rows.is_empty() {
        let now = now_ms();
        let placeholders = rows.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE restore_requests SET status = 'sent', sent_ms = ?1 WHERE id IN ({placeholders})"
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(rows.len() + 1);
        params.push(Box::new(now));
        for (id, _, _) in &rows {
            params.push(Box::new(*id));
        }
        db.conn()
            .execute(&sql, rusqlite::params_from_iter(params.iter().map(|b| b.as_ref())))?;
    }
    Ok(rows)
}

/// Marks a restore job complete.
pub fn finish_restore_request(db: &Db, id: i64, error: Option<&str>) -> Result<()> {
    db.conn().execute(
        "UPDATE restore_requests SET status = ?1, done_ms = ?2, error = ?3 WHERE id = ?4",
        rusqlite::params![
            if error.is_some() { "failed" } else { "done" },
            now_ms(),
            error,
            id
        ],
    )?;
    Ok(())
}

/// Recent restore jobs, newest first (diagnostics screen).
pub fn recent_restore_requests(
    db: &Db,
    limit: u32,
) -> Result<Vec<(i64, String, String, String, Option<i64>)>> {
    let mut stmt = db.conn().prepare(
        "SELECT id, action, status, COALESCE(error, ''), done_ms
         FROM restore_requests ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit as i64], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Backup helper: checkpoint the WAL then copy the database file.
pub fn backup_database(db: &Db, dest: &Path) -> Result<std::path::PathBuf> {
    db.checkpoint()?;
    let source_path = db.conn().path().map(std::path::PathBuf::from).ok_or_else(|| {
        crate::error::Error::Other("database is not attached to a file".into())
    })?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| crate::error::Error::io(parent.display(), e))?;
    }
    std::fs::copy(&source_path, dest).map_err(|e| {
        crate::error::Error::Other(format!(
            "backup from {} to {} failed: {e}",
            source_path.display(),
            dest.display()
        ))
    })?;
    Ok(dest.to_path_buf())
}
