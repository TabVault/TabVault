//! Read-side queries for the UI.
//!
//! Every list end-point takes a filter struct and returns rows with an explicit
//! `limit`/`offset`, so 100k+ records scroll smoothly through a virtualized
//! list instead of being materialized. All filters are parameterised.

use crate::error::Result;
use crate::model::{BrowserSnapshot, TabRecord, WindowSnapshot};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shared row shapes (serialized to the frontend)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClosedTabRow {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub favicon: Option<String>,
    pub domain: String,
    pub pinned: bool,
    pub browser: String,
    pub profile: String,
    pub opened_ms: Option<i64>,
    pub closed_ms: i64,
    pub last_active_ms: Option<i64>,
    pub active_ms: i64,
    pub visit_count: i64,
    pub group_name: Option<String>,
    pub group_color: Option<String>,
    pub part_of_closed_window: bool,
    pub source: String,
    pub confidence: String,
    pub window_event_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedTabPage {
    pub rows: Vec<ClosedTabRow>,
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ClosedWindowRow {
    pub id: i64,
    pub browser: String,
    pub profile: String,
    pub window_id: String,
    pub closed_ms: i64,
    pub tab_count: i64,
    pub pinned_count: i64,
    pub source: String,
    pub snapshot_id: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedTabFilter {
    /// Free text matched against title, URL and domain.
    pub q: Option<String>,
    /// Only pinned tabs.
    pub pinned_only: bool,
    /// Only a given browser (`chrome`, `edge`, `firefox`).
    pub browser: Option<String>,
    /// Only rows from this source (`extension`, `chromium_snss`, ...).
    pub source: Option<String>,
    /// Unix ms lower bound on the close time (inclusive).
    pub date_from: Option<i64>,
    /// Unix ms upper bound on the close time (exclusive).
    pub date_to: Option<i64>,
    /// Minimum active ms accumulated on the page.
    pub min_active_ms: Option<i64>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl ClosedTabFilter {
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(200).clamp(1, 1000)
    }
    pub fn offset(&self) -> u32 {
        self.offset.unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Recently Closed
// ---------------------------------------------------------------------------

pub fn list_closed_tabs(conn: &Connection, filter: &ClosedTabFilter) -> Result<ClosedTabPage> {
    let mut where_sql = "WHERE 1=1".to_string();
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(q) = &filter.q {
        let trimmed = q.trim();
        if !trimmed.is_empty() {
            where_sql.push_str(
                " AND (title LIKE ? ESCAPE '\\' OR url LIKE ? ESCAPE '\\'
                      OR url_key LIKE ? ESCAPE '\\' OR browser LIKE ? ESCAPE '\\')",
            );
            let like = format!("%{}%", escape_like(trimmed));
            for _ in 0..4 {
                binds.push(Box::new(like.clone()));
            }
        }
    }
    if filter.pinned_only {
        where_sql.push_str(" AND pinned = 1");
    }
    if let Some(b) = &filter.browser {
        where_sql.push_str(" AND browser = ?");
        binds.push(Box::new(b.clone()));
    }
    if let Some(s) = &filter.source {
        where_sql.push_str(" AND source = ?");
        binds.push(Box::new(s.clone()));
    }
    if let Some(from) = filter.date_from {
        where_sql.push_str(" AND closed_ms >= ?");
        binds.push(Box::new(from));
    }
    if let Some(to) = filter.date_to {
        where_sql.push_str(" AND closed_ms < ?");
        binds.push(Box::new(to));
    }
    if let Some(min_active) = filter.min_active_ms {
        where_sql.push_str(" AND active_ms >= ?");
        binds.push(Box::new(min_active));
    }

    let total: i64 = conn.query_row(
        &format!("SELECT count(*) FROM closed_tab_events {where_sql}"),
        rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())),
        |r| r.get(0),
    )?;

    let limit_clause = format!("{} ", filter.limit());
    let offset_clause = filter.offset();
    let sql = format!(
        "SELECT id, url, title, favicon, pinned, browser, profile, opened_ms, closed_ms,
                last_active_ms, active_ms, visit_count, group_name, group_color,
                part_of_closed_window, source, confidence, window_event_id
         FROM closed_tab_events {where_sql}
         ORDER BY closed_ms DESC, id DESC
         LIMIT {limit_clause}OFFSET {offset_clause}"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), |r| {
            let url: String = r.get(1)?;
            Ok(ClosedTabRow {
                id: r.get(0)?,
                title: r.get(2)?,
                favicon: r.get(3)?,
                domain: crate::url_util::registrable_domain(&url),
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
                part_of_closed_window: r.get::<_, i64>(14)? != 0,
                source: r.get(15)?,
                confidence: r.get(16)?,
                window_event_id: r.get(17)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ClosedTabPage { rows, total })
}

fn escape_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    for ch in input.chars() {
        if ch == '\\' || ch == '%' || ch == '_' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------
// Closed windows
// ---------------------------------------------------------------------------

pub fn list_closed_windows(
    conn: &Connection,
    browser: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<Vec<ClosedWindowRow>> {
    let mut sql = String::from(
        "SELECT id, browser, profile, window_id, closed_ms, tab_count, pinned_count, source, snapshot_id
         FROM closed_window_events",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(b) = browser {
        sql.push_str(" WHERE browser = ?");
        binds.push(Box::new(b.to_string()));
    }
    sql.push_str(" ORDER BY closed_ms DESC LIMIT ? OFFSET ?");
    let l = limit.clamp(1, 500);
    let o = offset;
    binds.push(Box::new(l as i64));
    binds.push(Box::new(o as i64));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), |r| {
            Ok(ClosedWindowRow {
                id: r.get(0)?,
                browser: r.get(1)?,
                profile: r.get(2)?,
                window_id: r.get(3)?,
                closed_ms: r.get(4)?,
                tab_count: r.get(5)?,
                pinned_count: r.get(6)?,
                source: r.get(7)?,
                snapshot_id: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Tabs belonging to a closed window, in visual order.
pub fn closed_window_tabs(conn: &Connection, window_event_id: i64) -> Result<Vec<ClosedTabRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, favicon, pinned, browser, profile, opened_ms, closed_ms,
                last_active_ms, active_ms, visit_count, group_name, group_color,
                part_of_closed_window, source, confidence, window_event_id
         FROM closed_tab_events WHERE window_event_id = ?1
         ORDER BY tab_index ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([window_event_id], |r| {
            let url: String = r.get(1)?;
            Ok(ClosedTabRow {
                id: r.get(0)?,
                title: r.get(2)?,
                favicon: r.get(3)?,
                domain: crate::url_util::registrable_domain(&url),
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
                part_of_closed_window: r.get::<_, i64>(14)? != 0,
                source: r.get(15)?,
                confidence: r.get(16)?,
                window_event_id: r.get(17)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Snapshots / timeline
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotRow {
    pub id: i64,
    pub taken_ms: i64,
    pub browser: String,
    pub profile: String,
    pub tab_count: i64,
    pub window_count: i64,
    pub reason: String,
    pub recovery_point: bool,
    pub dropped_count: i64,
}

pub fn list_snapshots(
    conn: &Connection,
    from: Option<i64>,
    to: Option<i64>,
    browser: Option<&str>,
    limit: u32,
    offset: u32,
) -> Result<Vec<SnapshotRow>> {
    let mut sql = String::from(
        "SELECT id, taken_ms, browser, profile, tab_count, window_count, reason, recovery_point, dropped_count
         FROM snapshots WHERE 1=1",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(f) = from {
        sql.push_str(" AND taken_ms >= ?");
        binds.push(Box::new(f));
    }
    if let Some(t) = to {
        sql.push_str(" AND taken_ms < ?");
        binds.push(Box::new(t));
    }
    if let Some(b) = browser {
        sql.push_str(" AND browser = ?");
        binds.push(Box::new(b.to_string()));
    }
    sql.push_str(" ORDER BY taken_ms DESC LIMIT ? OFFSET ?");
    binds.push(Box::new(limit.clamp(1, 1000) as i64));
    binds.push(Box::new(offset as i64));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), |r| {
            Ok(SnapshotRow {
                id: r.get(0)?,
                taken_ms: r.get(1)?,
                browser: r.get(2)?,
                profile: r.get(3)?,
                tab_count: r.get(4)?,
                window_count: r.get(5)?,
                reason: r.get(6)?,
                recovery_point: r.get::<_, i64>(7)? != 0,
                dropped_count: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Full reconstruction of a snapshot for the timeline preview/restore.
pub fn snapshot_detail(conn: &Connection, snapshot_id: i64) -> Result<Option<BrowserSnapshot>> {
    let Some(row) = conn
        .query_row(
            "SELECT browser, profile FROM snapshots WHERE id = ?1",
            [snapshot_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .ok()
    else {
        return Ok(None);
    };
    let (browser, profile) = row;

    let mut windows_stmt = conn.prepare(
        "SELECT window_id, focused, incognito, tab_count FROM windows WHERE snapshot_id = ?1",
    )?;
    let window_rows = windows_stmt
        .query_map([snapshot_id], |r| {
            Ok(WindowSnapshot {
                window_id: r.get(0)?,
                focused: r.get::<_, i64>(1)? != 0,
                incognito: r.get::<_, i64>(2)? != 0,
                tabs: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Windows keyed in insertion order of the table.
    let mut windows: Vec<WindowSnapshot> = window_rows;
    let mut tab_stmt = conn.prepare(
        "SELECT w.window_id, t.url, t.title, t.favicon, t.pinned, t.tab_index,
                t.group_name, t.group_color, t.opened_ms, t.closed_ms, t.last_active_ms,
                t.active_ms, t.visit_count, t.part_of_closed_window, t.incognito
         FROM tabs t JOIN windows w ON w.id = t.window_row_id
         WHERE t.snapshot_id = ?1
         ORDER BY w.window_id, t.tab_index",
    )?;
    let tabs = tab_stmt
        .query_map([snapshot_id], |r| {
            let window_id: String = r.get(0)?;
            Ok((window_id, TabRecord {
                url: r.get(1)?,
                title: r.get(2)?,
                favicon: r.get(3)?,
                pinned: r.get::<_, i64>(4)? != 0,
                tab_index: r.get(5)?,
                window_id: String::new(),
                group_name: r.get(6)?,
                group_color: r.get(7)?,
                browser: browser.clone(),
                profile: profile.clone(),
                opened_ms: r.get(8)?,
                closed_ms: r.get(9)?,
                last_active_ms: r.get(10)?,
                active_ms: r.get(11)?,
                visit_count: r.get(12)?,
                part_of_closed_window: r.get::<_, i64>(13)? != 0,
                incognito: r.get::<_, i64>(14)? != 0,
                source: crate::model::Source::Extension,
                confidence: crate::model::Confidence::High,
                browser_tab_id: None,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    for (window_id, mut tab) in tabs {
        tab.window_id = window_id.clone();
        if let Some(w) = windows.iter_mut().find(|w| w.window_id == window_id) {
            w.tabs.push(tab);
        }
    }
    // Stable visual order.
    for w in &mut windows {
        w.tabs.sort_by_key(|t| t.tab_index);
    }
    Ok(Some(BrowserSnapshot {
        browser,
        profile,
        windows,
    }))
}

// ---------------------------------------------------------------------------
// Most Used
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteStatRow {
    pub id: i64,
    pub url: String,
    pub url_key: String,
    pub domain: String,
    pub title: String,
    pub favicon: Option<String>,
    pub visit_count: i64,
    pub active_ms: i64,
    pub last_seen_ms: i64,
    pub first_seen_ms: i64,
    pub pinned: bool,
}

/// `window`: `day` | `week` | `month` | `all`.
pub fn top_sites(
    conn: &Connection,
    window: &str,
    metric: &str,
    limit: u32,
) -> Result<Vec<SiteStatRow>> {
    let since = match window {
        "day" => Some(crate::time::now_ms() - 86_400_000),
        "week" => Some(crate::time::now_ms() - 7 * 86_400_000),
        "month" => Some(crate::time::now_ms() - 30 * 86_400_000),
        _ => None,
    };
    let order = match metric {
        "active" => "active_ms DESC",
        _ => "visit_count DESC, active_ms DESC",
    };
    let mut sql = format!(
        "SELECT id, url, url_key, domain, title, favicon, visit_count, active_ms, last_seen_ms, first_seen_ms, pinned
         FROM visit_stats WHERE 1=1"
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(s) = since {
        sql.push_str(" AND last_seen_ms >= ?");
        binds.push(Box::new(s));
    }
    sql.push_str(&format!(" ORDER BY {order} LIMIT ?"));
    binds.push(Box::new(limit.clamp(1, 500) as i64));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), |r| {
            Ok(SiteStatRow {
                id: r.get(0)?,
                url: r.get(1)?,
                url_key: r.get(2)?,
                domain: r.get(3)?,
                title: r.get(4)?,
                favicon: r.get(5)?,
                visit_count: r.get(6)?,
                active_ms: r.get(7)?,
                last_seen_ms: r.get(8)?,
                first_seen_ms: r.get(9)?,
                pinned: r.get::<_, i64>(10)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Pinned Vault
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedSiteRow {
    pub id: i64,
    pub url: String,
    pub domain: String,
    pub title: String,
    pub favicon: Option<String>,
    pub browser: String,
    pub profile: String,
    pub first_pinned_ms: i64,
    pub last_pinned_ms: i64,
    pub pin_count: i64,
    pub currently_pinned: bool,
    pub group_name: Option<String>,
}

pub fn list_pinned_sites(conn: &Connection, browser: Option<&str>) -> Result<Vec<PinnedSiteRow>> {
    let mut sql = String::from(
        "SELECT id, url, domain, title, favicon, browser, profile, first_pinned_ms, last_pinned_ms, pin_count, currently_pinned, group_name
         FROM pinned_sites",
    );
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(b) = browser {
        sql.push_str(" WHERE browser = ?");
        binds.push(Box::new(b.to_string()));
    }
    sql.push_str(" ORDER BY last_pinned_ms DESC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())), |r| {
            Ok(PinnedSiteRow {
                id: r.get(0)?,
                url: r.get(1)?,
                domain: r.get(2)?,
                title: r.get(3)?,
                favicon: r.get(4)?,
                browser: r.get(5)?,
                profile: r.get(6)?,
                first_pinned_ms: r.get(7)?,
                last_pinned_ms: r.get(8)?,
                pin_count: r.get(9)?,
                currently_pinned: r.get::<_, i64>(10)? != 0,
                group_name: r.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Global search (Ctrl+K)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub url: String,
    pub domain: String,
    pub title: String,
    pub kind: String,
    pub pinned: bool,
    pub visit_count: i64,
    pub active_ms: i64,
    pub last_seen_ms: i64,
}

/// Turns free-text input into a safe FTS5 `AND` of prefix terms.
///
/// Anything that is not a word character is a separator; each token becomes a
/// quoted prefix term, so odd input can never become a syntax error.
fn fts_query(input: &str) -> Option<String> {
    let tokens: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(16)
        .map(|t| t.to_ascii_lowercase())
        .collect();
    if tokens.is_empty() {
        return None;
    }
    Some(
        tokens
            .iter()
            .map(|t| format!("\"{}\"*", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" AND "),
    )
}

pub fn global_search(conn: &Connection, query: &str, limit: u32) -> Result<Vec<SearchHit>> {
    let Some(fts) = fts_query(query) else {
        return Ok(Vec::new());
    };
    let limit = limit.clamp(1, 200);
    let mut stmt = conn.prepare(
        "SELECT d.url, d.domain, d.title, d.kind, d.pinned, d.visit_count, d.active_ms, d.last_seen_ms
         FROM search_fts f JOIN search_docs d ON d.id = f.rowid
         WHERE search_fts MATCH ?1
         ORDER BY (d.visit_count + d.active_ms / 60000) DESC, d.last_seen_ms DESC
         LIMIT ?2",
    )?;
    let hits = stmt
        .query_map(rusqlite::params![fts, limit as i64], |r| {
            Ok(SearchHit {
                url: r.get(0)?,
                domain: r.get(1)?,
                title: r.get(2)?,
                kind: r.get(3)?,
                pinned: r.get::<_, i64>(4)? != 0,
                visit_count: r.get(5)?,
                active_ms: r.get(6)?,
                last_seen_ms: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(hits)
}

// ---------------------------------------------------------------------------
// Dashboard stats & browsers list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardStats {
    pub snapshots: i64,
    pub closed_tabs: i64,
    pub closed_windows: i64,
    pub pinned_sites: i64,
    pub visit_est: i64,
    pub first_snapshot_ms: Option<i64>,
    pub last_snapshot_ms: Option<i64>,
    pub recovery_points: i64,
}

pub fn dashboard_stats(conn: &Connection) -> Result<DashboardStats> {
    let snapshots: i64 = conn.query_row("SELECT count(*) FROM snapshots", [], |r| r.get(0))?;
    let closed_tabs: i64 =
        conn.query_row("SELECT count(*) FROM closed_tab_events", [], |r| r.get(0))?;
    let closed_windows: i64 =
        conn.query_row("SELECT count(*) FROM closed_window_events", [], |r| r.get(0))?;
    let pinned_sites: i64 =
        conn.query_row("SELECT count(*) FROM pinned_sites", [], |r| r.get(0))?;
    let visit_est: i64 =
        conn.query_row("SELECT COALESCE(sum(visit_count), 0) FROM visit_stats", [], |r| {
            r.get(0)
        })?;
    let first: Option<i64> =
        conn.query_row("SELECT MIN(taken_ms) FROM snapshots", [], |r| r.get(0))?;
    let last: Option<i64> =
        conn.query_row("SELECT MAX(taken_ms) FROM snapshots", [], |r| r.get(0))?;
    let recovery_points: i64 =
        conn.query_row("SELECT count(*) FROM snapshots WHERE recovery_point = 1", [], |r| {
            r.get(0)
        })?;
    Ok(DashboardStats {
        snapshots,
        closed_tabs,
        closed_windows,
        pinned_sites,
        visit_est,
        first_snapshot_ms: first,
        last_snapshot_ms: last,
        recovery_points,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserProfileRow {
    pub browser: String,
    pub profile: String,
    pub snapshots: i64,
    pub last_snapshot_ms: Option<i64>,
}

pub fn browser_profiles(conn: &Connection) -> Result<Vec<BrowserProfileRow>> {
    let mut stmt = conn.prepare(
        "SELECT browser, profile, count(*), MAX(taken_ms)
         FROM snapshots GROUP BY browser, profile ORDER BY 4 DESC",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(BrowserProfileRow {
                browser: r.get(0)?,
                profile: r.get(1)?,
                snapshots: r.get(2)?,
                last_snapshot_ms: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
