//! Chromium `History` database importer.
//!
//! Chrome and Edge store their history in a SQLite file named `History` inside
//! the profile. The live browser holds the file open with SHARE_DENY_WRITE, and
//! its WAL can be mid-checkpoint at any moment — so per the spec we **copy the
//! file and its `-wal`/`-shm` siblings to a temporary directory first**, then
//! open the copy read-only. The original profile is never touched.
//!
//! The `urls` table gives us `url`, `title`, `visit_count`, and
//! `last_visit_time` (a `base::Time` in **microseconds since 1601**), which is
//! exactly what the "Most Used" report needs when it is seeded from history.
//! History is a *visit* record, not a snapshot record, so confidence is always
//! [`Confidence::Low`] and the rows land in `visit_stats`, never as "tabs that
//! were open".

use crate::error::Result;
use crate::model::{Confidence, Source, TabRecord};
use crate::time::windows_micros_to_unix_ms;

/// One row from the `urls` table.
#[derive(Debug, Clone, PartialEq)]
pub struct ChromiumHistoryRow {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    /// Unix milliseconds, normalised at parse time.
    pub last_visit_ms: Option<i64>,
}

impl ChromiumHistoryRow {
    /// History rows have no pinned/open semantics; they inform statistics.
    pub fn to_tab_record(&self, browser: &str, profile: &str) -> TabRecord {
        TabRecord {
            url: self.url.clone(),
            title: self.title.clone(),
            favicon: None,
            pinned: false,
            tab_index: 0,
            window_id: String::new(),
            group_name: None,
            group_color: None,
            browser: browser.to_string(),
            profile: profile.to_string(),
            opened_ms: None,
            closed_ms: None,
            last_active_ms: self.last_visit_ms,
            active_ms: 0,
            visit_count: self.visit_count,
            part_of_closed_window: false,
            incognito: false,
            source: Source::ChromiumHistory,
            confidence: Confidence::Low,
        }
    }
}

/// Reads the `urls` table of a (copied) Chromium History database.
pub fn read_chromium_history(conn: &rusqlite::Connection) -> Result<Vec<ChromiumHistoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT url, COALESCE(title, ''), visit_count, last_visit_time
         FROM urls
         WHERE visit_count > 0
         ORDER BY last_visit_time DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ChromiumHistoryRow {
            url: r.get(0)?,
            title: r.get(1)?,
            visit_count: r.get(2)?,
            last_visit_ms: r.get::<_, i64>(3).ok().and_then(windows_micros_to_unix_ms),
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}
