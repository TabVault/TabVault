//! Database access.
//!
//! One connection is opened per process. Two processes use the file at the same
//! time by design — `tabvault-host` (spawned by the browser) writes events, and
//! the Tauri app reads them — so the database is always in WAL mode with a busy
//! timeout. WAL gives us readers that never block the writer, and the
//! crash-safety we need; the timeout turns the rare writer/writer overlap into a
//! short wait instead of an immediate `SQLITE_BUSY`.

pub mod query;
pub mod schema;
pub mod write;

use crate::error::{Error, Result};
use crate::model::Settings;
use rusqlite::Connection;
use std::path::Path;

/// A handle to the vault.
///
/// `rusqlite::Connection` is `Send` but not `Sync`, so the app wraps this in a
/// `Mutex`; the host uses it single-threaded.
pub struct Db {
    conn: Connection,
    /// True when the file is SQLCipher-encrypted.
    encrypted: bool,
}

/// Tuning applied to every connection.
const BUSY_TIMEOUT_MS: u32 = 5_000;
/// 64 MiB of page cache: comfortably covers the hot working set of a 100k-row
/// vault without being a meaningful share of the RAM budget.
const CACHE_SIZE_KIB: i64 = -65_536;
/// Memory-map 256 MiB so indexed reads skip the read() syscall.
const MMAP_SIZE: i64 = 268_435_456;

impl Db {
    /// Opens (creating if needed) the vault at `path`.
    ///
    /// When `key` is `Some`, SQLCipher encrypts the file. The key must be set
    /// before any other statement, which is why it is handled here rather than by
    /// the caller.
    pub fn open(path: &Path, key: Option<&str>) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent.display(), e))?;
        }
        let conn = Connection::open(path)?;
        Self::configure(conn, key)
    }

    /// Wraps an already-open connection (used by `backup`, which needs to
    /// manage its own destination connection).
    pub fn configure(conn: Connection, key: Option<&str>) -> Result<Self> {
        // `PRAGMA key` must come first, before anything touches the file header.
        if let Some(k) = key {
            if !k.is_empty() {
                conn.pragma_update(None, "key", k)?;
                // Verify the key immediately: SQLCipher returns "file is not a
                // database" on the first real read, and we want that error here
                // where we can report it as a wrong-password condition.
                conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
                    .map_err(|e| match e {
                        rusqlite::Error::SqliteFailure(..) => Error::Other(
                            "the database is encrypted and the key was rejected".to_string(),
                        ),
                        other => Error::Sqlite(other),
                    })?;
            }
        }

        let encrypted = key.map(|k| !k.is_empty()).unwrap_or(false);

        // WAL is requested on the connection but is a *persistent* property of
        // the file, so this is a no-op after the first open.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // NORMAL is the correct pairing with WAL: fsync only at checkpoints,
        // which keeps the extension's writes off the critical path while still
        // surviving a process crash. FULL would be needed only to survive an OS
        // crash mid-checkpoint.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS as u64))?;
        conn.pragma_update(None, "cache_size", CACHE_SIZE_KIB)?;
        conn.pragma_update(None, "mmap_size", MMAP_SIZE)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        // Cap the WAL so a long-running session cannot accumulate an unbounded
        // -wal file (which also matters because we back the file up).
        conn.pragma_update(None, "wal_autocheckpoint", 1_000)?;

        let db = Db { conn, encrypted };
        db.migrate()?;
        Ok(db)
    }

    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Applies any migrations the file has not seen yet.
    ///
    /// Runs each pending migration in its own transaction with the version bump,
    /// so an interrupted upgrade is atomic.
    pub fn migrate(&self) -> Result<()> {
        let current: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(max(version), 0) FROM sqlite_master \
                 WHERE type='table' AND name='schema_migrations'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|table_exists| {
                if table_exists == 0 {
                    return 0;
                }
                self.conn
                    .query_row(
                        "SELECT COALESCE(max(version), 0) FROM schema_migrations",
                        [],
                        |r| r.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
            })
            .unwrap_or(0);

        if current == 0 {
            // The migrations table itself is created by migration 1, so we must
            // not query it before running anything.
            self.run_migration(&schema::MIGRATIONS[0])?;
        }

        for m in schema::MIGRATIONS.iter().filter(|m| m.version > current) {
            let applied: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
                [m.version],
                |r| r.get(0),
            )?;
            if applied {
                continue;
            }
            self.run_migration(m)?;
            tracing::info!(version = m.version, name = m.name, "applied migration");
        }
        Ok(())
    }

    fn run_migration(&self, m: &schema::Migration) -> Result<()> {
        self.conn.execute_batch(&format!(
            "BEGIN IMMEDIATE;\n{}\nINSERT OR REPLACE INTO schema_migrations (version, name, applied_ms) \
             VALUES ({}, '{}', {});\nCOMMIT;",
            m.sql,
            m.version,
            m.name,
            crate::time::now_ms(),
        ))?;
        Ok(())
    }

    /// Reads a JSON setting, falling back to the default when absent or corrupt.
    pub fn get_setting<T: serde::de::DeserializeOwned + Default>(&self, key: &str) -> T {
        match self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get::<_, String>(0)
            }) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => T::default(),
        }
    }

    pub fn set_setting<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let json = serde_json::to_string(value)?;
        self.conn.execute(
            "INSERT INTO settings (key, value, updated_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_ms = excluded.updated_ms",
            rusqlite::params![key, json, crate::time::now_ms()],
        )?;
        Ok(())
    }

    /// Convenience wrapper for the single app-wide settings document.
    pub fn settings(&self) -> Settings {
        let mut s: Settings = self.get_setting("app");
        s.sanitize();
        s
    }

    pub fn set_settings(&self, s: &Settings) -> Result<()> {
        let mut s = s.clone();
        s.sanitize();
        self.set_setting("app", &s)
    }

    /// `PRAGMA integrity_check`, used before/after an import and by the
    /// diagnostics screen.
    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Flushes the WAL into the main file. Called before a backup and on
    /// graceful shutdown so the copied file is self-contained.
    pub fn checkpoint(&self) -> Result<()> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .or_else(|e| match e {
                // A checkpoint can fail with SQLITE_BUSY while another process
                // holds a read transaction; that is not fatal for a backup,
                // because we copy the -wal alongside.
                rusqlite::Error::SqliteFailure(..) => Ok(()),
                other => Err(other),
            })?;
        Ok(())
    }

    /// Approximate on-disk size of the vault, including its WAL.
    pub fn disk_usage(&self) -> Result<u64> {
        let page_count: i64 = self
            .conn
            .query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = self
            .conn
            .query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok((page_count.max(0) as u64).saturating_mul(page_size.max(0) as u64))
    }
}

/// How the vault file should be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultMode {
    /// Ordinary plaintext vault.
    Plain,
    /// SQLCipher-encrypted; the key comes from a DPAPI-wrapped blob next to
    /// the database (`key.dpapi`).
    Encrypted,
}

/// Reads the DPAPI-wrapped vault key from `<db path>/../key.dpapi`.
///
/// Returns hex-encoded key material suitable for `PRAGMA key`.
fn read_dpapi_key(db_path: &Path) -> Result<String> {
    let key_path = db_path
        .parent()
        .map(|p| p.join("key.dpapi"))
        .ok_or_else(|| Error::Other("database path has no parent directory".into()))?;
    let blob = std::fs::read(&key_path).map_err(|e| {
        Error::Other(format!(
            "could not read the vault key at {}: {e}. The database is encrypted; the desktop app must be reinstalled or the key recovered from backup.",
            key_path.display()
        ))
    })?;
    let raw = crate::crypto::dpapi_unprotect(&blob)?;
    if raw.len() < 16 {
        return Err(Error::Other("the vault key blob is too short".into()));
    }
    Ok(raw.iter().map(|b| format!("{b:02x}")).collect())
}

/// Opens (creating if needed) the vault, picking plain or encrypted mode from
/// the presence of the DPAPI key file. TabVault's on-disk format never
/// requires the caller to know which mode is in use.
pub fn open_vault(path: &Path) -> Result<Db> {
    let encrypted = path
        .parent()
        .map(|p| p.join("key.dpapi").is_file())
        .unwrap_or(false);
    let key = if encrypted {
        Some(read_dpapi_key(path)?)
    } else {
        None
    };
    Db::open(path, key.as_deref())
}

/// Normalised key for a URL, used for every de-duplication and grouping query.
///
/// Rules, in order: trim; drop the fragment (it never identifies a different
/// page for our purposes); lower-case the scheme and host; remove a trailing
/// slash on a bare root path; strip a default port. Query strings are kept,
/// because `?id=1` and `?id=2` really are different pages.
pub fn url_key(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    let (without_frag, _) = match url.split_once('#') {
        Some((a, _)) => (a, true),
        None => (url, false),
    };
    let Some((scheme, rest)) = without_frag.split_once("://") else {
        return without_frag.to_ascii_lowercase();
    };
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path_and_query) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    // Lower-case the host but preserve userinfo-free authority exactly, minus a
    // default port.
    let authority = {
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p))
                if p.chars().all(|c| c.is_ascii_digit())
                    && !(p.is_empty() || p == "80" || p == "443") =>
            {
                (h, Some(p))
            }
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, None),
            _ => (authority, None),
        };
        match port {
            Some(p) => format!("{}:{p}", host.to_ascii_lowercase()),
            None => host.to_ascii_lowercase(),
        }
    };
    let path = if path_and_query.is_empty() || path_and_query == "/" {
        ""
    } else {
        path_and_query
    };
    format!("{scheme}://{authority}{path}")
}

/// Splits a URL into words for the FTS index, so that a search for "github"
/// matches `https://github.com/rust-lang/rust`.
pub fn url_words(url: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in url.chars() {
        if ch.is_ascii_alphanumeric() {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    // Drop the scheme token; it is pure noise in search results.
    words.retain(|w| !matches!(w.as_str(), "http" | "https" | "www" | "com" | "org" | "net"));
    words.dedup();
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_key_normalises_but_preserves_identity() {
        assert_eq!(url_key("https://Example.COM/"), "https://example.com");
        assert_eq!(url_key("https://example.com"), "https://example.com");
        assert_eq!(url_key("https://example.com/path"), "https://example.com/path");
        assert_eq!(
            url_key("https://example.com/path#section"),
            "https://example.com/path"
        );
        assert_eq!(url_key("https://example.com:443/x"), "https://example.com/x");
        assert_eq!(url_key("https://example.com:8443/x"), "https://example.com:8443/x");
        // Query strings identify distinct pages and must survive.
        assert_ne!(url_key("https://e.com/?id=1"), url_key("https://e.com/?id=2"));
        assert_eq!(url_key("  https://e.com/a  "), "https://e.com/a");
        assert_eq!(url_key(""), "");
        assert_eq!(url_key("not-a-url"), "not-a-url");
    }

    #[test]
    fn url_words_extracts_searchable_tokens() {
        let w = url_words("https://github.com/rust-lang/rust/issues?q=async");
        assert!(w.contains("github"));
        assert!(w.contains("rust"));
        assert!(w.contains("issues"));
        assert!(w.contains("async"));
        // Scheme and noise TLD tokens are dropped.
        assert!(!w.contains("https"));
        assert!(!w.contains("com"));
    }

    #[test]
    fn migrations_apply_to_a_fresh_database_and_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.db");
        let db = Db::open(&path, None).unwrap();
        let v: i64 = db
            .conn()
            .query_row("SELECT max(version) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 2);
        // Re-opening must not re-run or fail.
        drop(db);
        let db = Db::open(&path, None).unwrap();
        let count: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn wal_mode_and_foreign_keys_are_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("vault.db"), None).unwrap();
        let jm: String = db
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(jm.to_ascii_lowercase(), "wal");
        let fk: i64 = db
            .conn()
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn settings_roundtrip_and_default_on_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("vault.db"), None).unwrap();
        let mut s = db.settings();
        s.capture_interval_secs = 30;
        db.set_settings(&s).unwrap();
        assert_eq!(db.settings().capture_interval_secs, 30);

        db.conn()
            .execute(
                "UPDATE settings SET value = 'not json' WHERE key = 'app'",
                [],
            )
            .unwrap();
        // A corrupt setting must not take the app down.
        assert_eq!(db.settings().capture_interval_secs, 60);
    }

    #[test]
    fn integrity_check_passes_on_a_new_vault() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("vault.db"), None).unwrap();
        assert_eq!(db.integrity_check().unwrap(), vec!["ok".to_string()]);
    }
}
