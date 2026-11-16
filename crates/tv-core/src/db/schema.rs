//! Schema and migrations.
//!
//! Migrations are append-only: never edit a shipped migration, always add a new
//! one. Each runs inside a transaction together with its version bump, so a
//! crash mid-upgrade leaves either the old or the new state, never a half-built
//! database.
//!
//! Storage model in one paragraph: `snapshots` holds one row per captured
//! instant, `windows` and `tabs` hold that instant's payload and cascade away
//! with their snapshot. `closed_tab_events` and `closed_window_events` are the
//! permanent, append-only history — snapshots are subject to retention but tab
//! closes are not, because that is the thing the user actually wants to keep.
//! `search_docs` + `search_fts` is a deduplicated, one-row-per-distinct-page
//! index used by Ctrl+K; it is deliberately *not* built from `tabs`, which would
//! duplicate every snapshot's worth of rows.

/// A single migration step.
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// The full ordered migration list.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial_schema",
    sql: r#"
-- ---------------------------------------------------------------------------
-- Migration bookkeeping
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS schema_migrations (
    version   INTEGER PRIMARY KEY,
    name      TEXT    NOT NULL,
    applied_ms INTEGER NOT NULL
);

-- ---------------------------------------------------------------------------
-- Settings: one JSON document, so adding a setting never needs a migration.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_ms INTEGER NOT NULL
);

-- ---------------------------------------------------------------------------
-- Snapshots: one row per captured instant.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS snapshots (
    id             INTEGER PRIMARY KEY,
    taken_ms       INTEGER NOT NULL,
    browser        TEXT    NOT NULL,
    profile        TEXT    NOT NULL,
    fingerprint    TEXT    NOT NULL,
    tab_count      INTEGER NOT NULL DEFAULT 0,
    window_count   INTEGER NOT NULL DEFAULT 0,
    reason         TEXT    NOT NULL,
    recovery_point INTEGER NOT NULL DEFAULT 0,
    dropped_count  INTEGER NOT NULL DEFAULT 0,
    note           TEXT
);

-- Dedupe: an identical state for the same profile must not create a second row.
CREATE UNIQUE INDEX IF NOT EXISTS ux_snapshots_identity
    ON snapshots (browser, profile, fingerprint);
CREATE INDEX IF NOT EXISTS ix_snapshots_taken
    ON snapshots (taken_ms DESC);
CREATE INDEX IF NOT EXISTS ix_snapshots_profile_taken
    ON snapshots (browser, profile, taken_ms DESC);
CREATE INDEX IF NOT EXISTS ix_snapshots_recovery
    ON snapshots (taken_ms DESC) WHERE recovery_point = 1;

-- ---------------------------------------------------------------------------
-- Windows: payload of a snapshot.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS windows (
    id          INTEGER PRIMARY KEY,
    snapshot_id INTEGER NOT NULL REFERENCES snapshots (id) ON DELETE CASCADE,
    window_id   TEXT    NOT NULL,
    focused     INTEGER NOT NULL DEFAULT 0,
    incognito   INTEGER NOT NULL DEFAULT 0,
    tab_count   INTEGER NOT NULL DEFAULT 0,
    bounds_x    INTEGER,
    bounds_y    INTEGER,
    bounds_w    INTEGER,
    bounds_h    INTEGER,
    show_state  TEXT,
    UNIQUE (snapshot_id, window_id)
);
CREATE INDEX IF NOT EXISTS ix_windows_snapshot ON windows (snapshot_id);

-- ---------------------------------------------------------------------------
-- Tabs: payload of a snapshot, and rows produced by the session-file parsers.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tabs (
    id                   INTEGER PRIMARY KEY,
    snapshot_id          INTEGER NOT NULL REFERENCES snapshots (id) ON DELETE CASCADE,
    window_row_id        INTEGER NOT NULL REFERENCES windows (id) ON DELETE CASCADE,
    url                  TEXT    NOT NULL,
    url_key              TEXT    NOT NULL,
    title                TEXT    NOT NULL DEFAULT '',
    favicon              TEXT,
    pinned               INTEGER NOT NULL DEFAULT 0,
    tab_index            INTEGER NOT NULL DEFAULT 0,
    group_name           TEXT,
    group_color          TEXT,
    browser              TEXT    NOT NULL,
    profile              TEXT    NOT NULL,
    source               TEXT    NOT NULL DEFAULT 'extension',
    confidence           TEXT    NOT NULL DEFAULT 'high',
    opened_ms            INTEGER,
    closed_ms            INTEGER,
    last_active_ms       INTEGER,
    active_ms            INTEGER NOT NULL DEFAULT 0,
    visit_count          INTEGER NOT NULL DEFAULT 0,
    part_of_closed_window INTEGER NOT NULL DEFAULT 0,
    incognito            INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ix_tabs_snapshot   ON tabs (snapshot_id);
CREATE INDEX IF NOT EXISTS ix_tabs_window_row ON tabs (window_row_id);
CREATE INDEX IF NOT EXISTS ix_tabs_url_key    ON tabs (url_key);
CREATE INDEX IF NOT EXISTS ix_tabs_pinned     ON tabs (browser, pinned) WHERE pinned = 1;

-- ---------------------------------------------------------------------------
-- Closed windows: permanent history, never pruned by snapshot retention.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS closed_window_events (
    id           INTEGER PRIMARY KEY,
    browser      TEXT    NOT NULL,
    profile      TEXT    NOT NULL,
    window_id    TEXT    NOT NULL,
    closed_ms    INTEGER NOT NULL,
    tab_count    INTEGER NOT NULL DEFAULT 0,
    pinned_count INTEGER NOT NULL DEFAULT 0,
    snapshot_id  INTEGER REFERENCES snapshots (id) ON DELETE SET NULL,
    source       TEXT    NOT NULL DEFAULT 'extension',
    reason       TEXT
);
CREATE INDEX IF NOT EXISTS ix_closed_windows_closed
    ON closed_window_events (closed_ms DESC);
CREATE INDEX IF NOT EXISTS ix_closed_windows_profile
    ON closed_window_events (browser, profile, closed_ms DESC);

-- ---------------------------------------------------------------------------
-- Closed tabs: the single most used feature, so it gets the most indexes.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS closed_tab_events (
    id                    INTEGER PRIMARY KEY,
    browser               TEXT    NOT NULL,
    profile               TEXT    NOT NULL,
    url                   TEXT    NOT NULL,
    url_key               TEXT    NOT NULL,
    title                 TEXT    NOT NULL DEFAULT '',
    favicon               TEXT,
    pinned                INTEGER NOT NULL DEFAULT 0,
    tab_index             INTEGER NOT NULL DEFAULT 0,
    window_id             TEXT    NOT NULL DEFAULT '',
    group_name            TEXT,
    group_color           TEXT,
    opened_ms             INTEGER,
    closed_ms             INTEGER NOT NULL,
    last_active_ms        INTEGER,
    active_ms             INTEGER NOT NULL DEFAULT 0,
    visit_count           INTEGER NOT NULL DEFAULT 0,
    part_of_closed_window INTEGER NOT NULL DEFAULT 0,
    source                TEXT    NOT NULL DEFAULT 'extension',
    confidence            TEXT    NOT NULL DEFAULT 'high',
    window_event_id       INTEGER REFERENCES closed_window_events (id) ON DELETE SET NULL,
    restored_ms           INTEGER
);
CREATE INDEX IF NOT EXISTS ix_closed_tabs_closed       ON closed_tab_events (closed_ms DESC);
CREATE INDEX IF NOT EXISTS ix_closed_tabs_url_key      ON closed_tab_events (url_key);
CREATE INDEX IF NOT EXISTS ix_closed_tabs_profile_time ON closed_tab_events (browser, profile, closed_ms DESC);
CREATE INDEX IF NOT EXISTS ix_closed_tabs_pinned       ON closed_tab_events (browser, closed_ms DESC) WHERE pinned = 1;
CREATE INDEX IF NOT EXISTS ix_closed_tabs_window_event ON closed_tab_events (window_event_id);

-- ---------------------------------------------------------------------------
-- Visit statistics: aggregated per page, one row per (browser, profile, url).
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS visit_stats (
    id            INTEGER PRIMARY KEY,
    browser       TEXT    NOT NULL,
    profile       TEXT    NOT NULL,
    domain        TEXT    NOT NULL,
    url           TEXT    NOT NULL,
    url_key       TEXT    NOT NULL,
    title         TEXT    NOT NULL DEFAULT '',
    favicon       TEXT,
    visit_count   INTEGER NOT NULL DEFAULT 0,
    active_ms     INTEGER NOT NULL DEFAULT 0,
    first_seen_ms INTEGER NOT NULL,
    last_seen_ms  INTEGER NOT NULL,
    pinned        INTEGER NOT NULL DEFAULT 0,
    UNIQUE (browser, profile, url_key)
);
CREATE INDEX IF NOT EXISTS ix_visit_domain     ON visit_stats (browser, profile, domain);
CREATE INDEX IF NOT EXISTS ix_visit_last_seen  ON visit_stats (browser, profile, last_seen_ms DESC);
CREATE INDEX IF NOT EXISTS ix_visit_count      ON visit_stats (browser, profile, visit_count DESC);
CREATE INDEX IF NOT EXISTS ix_visit_active     ON visit_stats (browser, profile, active_ms DESC);

-- ---------------------------------------------------------------------------
-- Pinned vault: every site ever pinned, with its first/last pin dates.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS pinned_sites (
    id               INTEGER PRIMARY KEY,
    browser          TEXT    NOT NULL,
    profile          TEXT    NOT NULL,
    url              TEXT    NOT NULL,
    url_key          TEXT    NOT NULL,
    title            TEXT    NOT NULL DEFAULT '',
    favicon          TEXT,
    domain           TEXT    NOT NULL DEFAULT '',
    first_pinned_ms  INTEGER NOT NULL,
    last_pinned_ms   INTEGER NOT NULL,
    pin_count        INTEGER NOT NULL DEFAULT 1,
    currently_pinned INTEGER NOT NULL DEFAULT 0,
    group_name       TEXT,
    UNIQUE (browser, profile, url_key)
);
CREATE INDEX IF NOT EXISTS ix_pinned_current  ON pinned_sites (browser, currently_pinned) WHERE currently_pinned = 1;
CREATE INDEX IF NOT EXISTS ix_pinned_last     ON pinned_sites (last_pinned_ms DESC);

-- ---------------------------------------------------------------------------
-- Search index: one row per distinct page, kept in sync by triggers.
-- `url_words` is the URL split on punctuation so that word/prefix search finds
-- "github" inside "https://github.com/x/y" without a substring scan.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS search_docs (
    id           INTEGER PRIMARY KEY,
    doc_key      TEXT    NOT NULL UNIQUE,   -- browser|profile|url_key
    browser      TEXT    NOT NULL,
    profile      TEXT    NOT NULL,
    url          TEXT    NOT NULL,
    url_key      TEXT    NOT NULL,
    title        TEXT    NOT NULL DEFAULT '',
    domain       TEXT    NOT NULL DEFAULT '',
    url_words    TEXT    NOT NULL DEFAULT '',
    kind         TEXT    NOT NULL DEFAULT 'visited',
    pinned       INTEGER NOT NULL DEFAULT 0,
    visit_count  INTEGER NOT NULL DEFAULT 0,
    active_ms    INTEGER NOT NULL DEFAULT 0,
    last_seen_ms INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ix_search_docs_last_seen ON search_docs (last_seen_ms DESC);
CREATE INDEX IF NOT EXISTS ix_search_docs_url_key   ON search_docs (url_key);

CREATE VIRTUAL TABLE IF NOT EXISTS search_fts USING fts5 (
    title,
    url_words,
    domain,
    content = 'search_docs',
    content_rowid = 'id',
    tokenize = "unicode61 remove_diacritics 2"
);

CREATE TRIGGER IF NOT EXISTS trg_search_docs_ai AFTER INSERT ON search_docs BEGIN
    INSERT INTO search_fts (rowid, title, url_words, domain)
    VALUES (new.id, new.title, new.url_words, new.domain);
END;
CREATE TRIGGER IF NOT EXISTS trg_search_docs_ad AFTER DELETE ON search_docs BEGIN
    INSERT INTO search_fts (search_fts, rowid, title, url_words, domain)
    VALUES ('delete', old.id, old.title, old.url_words, old.domain);
END;
CREATE TRIGGER IF NOT EXISTS trg_search_docs_au AFTER UPDATE ON search_docs BEGIN
    INSERT INTO search_fts (search_fts, rowid, title, url_words, domain)
    VALUES ('delete', old.id, old.title, old.url_words, old.domain);
    INSERT INTO search_fts (rowid, title, url_words, domain)
    VALUES (new.id, new.title, new.url_words, new.domain);
END;

-- ---------------------------------------------------------------------------
-- Import batches: lets the UI group "recovered from file X at time T" and
-- undo a batch that produced junk.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS import_batches (
    id             INTEGER PRIMARY KEY,
    started_ms     INTEGER NOT NULL,
    finished_ms    INTEGER,
    source         TEXT    NOT NULL,
    source_path    TEXT,
    browser        TEXT,
    profile        TEXT,
    items_found    INTEGER NOT NULL DEFAULT 0,
    items_imported INTEGER NOT NULL DEFAULT 0,
    tables_used    TEXT,
    status         TEXT    NOT NULL DEFAULT 'running',
    error          TEXT
);
CREATE INDEX IF NOT EXISTS ix_import_batches_started ON import_batches (started_ms DESC);
"#,
},
Migration {
    version: 2,
    name: "live_state_and_restore",
    sql: r#"
-- ---------------------------------------------------------------------------
-- live_state: the last full state the host wrote for each browser+profile.
-- It lets the native host survive its own crash: on restart it diffs against
-- this row instead of relying on the extension's in-memory state.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS live_state (
    browser     TEXT NOT NULL,
    profile     TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    payload     TEXT NOT NULL,
    updated_ms  INTEGER NOT NULL,
    PRIMARY KEY (browser, profile)
);

-- ---------------------------------------------------------------------------
-- restore_requests: jobs the desktop app asks the browser to perform.
-- The extension polls for 'pending' jobs; the host forwards them.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS restore_requests (
    id         INTEGER PRIMARY KEY,
    created_ms INTEGER NOT NULL,
    action     TEXT NOT NULL,          -- tab|window|selection|snapshot|repin_all
    payload    TEXT NOT NULL,          -- JSON
    status     TEXT NOT NULL DEFAULT 'pending',   -- pending|sent|done|failed
    sent_ms    INTEGER,
    done_ms    INTEGER,
    error      TEXT
);
CREATE INDEX IF NOT EXISTS ix_restore_requests_status
    ON restore_requests (status, id);
"#,
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_ordered_and_unique() {
        let mut last = 0;
        for m in MIGRATIONS {
            assert!(
                m.version > last,
                "migration {} must be greater than {last}",
                m.version
            );
            last = m.version;
        }
    }

    #[test]
    fn migration_names_are_non_empty_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for m in MIGRATIONS {
            assert!(!m.name.is_empty());
            assert!(seen.insert(m.name), "duplicate migration name {}", m.name);
        }
    }
}
