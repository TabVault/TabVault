//! Shared data types.
//!
//! These are the wire/DB shape of everything the extension and the parsers
//! produce. Field names are deliberately explicit (`*_ms` for timestamps) since
//! this struct crosses three languages: TypeScript in the extension, JSON over
//! native messaging, and SQLite.

use serde::{Deserialize, Serialize};

/// Where a record came from. Kept per-row so the UI can tell the user how much
/// to trust a restored tab and so "recover from the past" results can be shown
/// separately from live captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Live capture by the browser extension.
    #[default]
    Extension,
    /// Chromium `Session_*` / `Tabs_*` SNSS file.
    ChromiumSnss,
    /// Firefox `sessionstore-backups/*.jsonlz4`.
    FirefoxSessionStore,
    /// Firefox `places.sqlite` history.
    FirefoxPlaces,
    /// Chromium `History` SQLite database.
    ChromiumHistory,
    /// User-supplied JSON/HTML import.
    Import,
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Extension => "extension",
            Source::ChromiumSnss => "chromium_snss",
            Source::FirefoxSessionStore => "firefox_sessionstore",
            Source::FirefoxPlaces => "firefox_places",
            Source::ChromiumHistory => "chromium_history",
            Source::Import => "import",
        }
    }

    pub fn parse(s: &str) -> Option<Source> {
        Some(match s {
            "extension" => Source::Extension,
            "chromium_snss" => Source::ChromiumSnss,
            "firefox_sessionstore" => Source::FirefoxSessionStore,
            "firefox_places" => Source::FirefoxPlaces,
            "chromium_history" => Source::ChromiumHistory,
            "import" => Source::Import,
            _ => return None,
        })
    }

    /// Human label for the UI badge.
    pub const fn label(self) -> &'static str {
        match self {
            Source::Extension => "Live capture",
            Source::ChromiumSnss => "Chrome/Edge session file",
            Source::FirefoxSessionStore => "Firefox session file",
            Source::FirefoxPlaces => "Firefox history",
            Source::ChromiumHistory => "Chrome/Edge history",
            Source::Import => "Imported",
        }
    }
}

/// How much we trust a record's *details* (not its URL, which is almost always
/// exact). SNSS is append-only, so a tab's pinned flag depends on whether a
/// `SetPinnedState` record was still in the file when we read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Details are authoritative (live capture, or a session file that still
    /// contained the tab's full record set).
    #[default]
    High,
    /// URL and title known; pinned/group inferred from surviving records.
    Medium,
    /// Only the URL survived (typically history-derived); treat as a lead.
    Low,
}

impl Confidence {
    pub const fn as_str(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }

    pub fn parse(s: &str) -> Option<Confidence> {
        Some(match s {
            "high" => Confidence::High,
            "medium" => Confidence::Medium,
            "low" => Confidence::Low,
            _ => return None,
        })
    }
}

/// A single tab, as captured, recovered, or imported.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TabRecord {
    pub url: String,
    pub title: String,
    pub favicon: Option<String>,
    pub pinned: bool,
    pub tab_index: i64,
    /// Stable-ish per-browser window identifier (extension `windowId` or the
    /// SNSS window session id as a string).
    pub window_id: String,
    pub group_name: Option<String>,
    pub group_color: Option<String>,
    pub browser: String,
    pub profile: String,
    pub opened_ms: Option<i64>,
    pub closed_ms: Option<i64>,
    pub last_active_ms: Option<i64>,
    pub active_ms: i64,
    pub visit_count: i64,
    pub part_of_closed_window: bool,
    pub incognito: bool,
    pub source: Source,
    pub confidence: Confidence,
}

impl TabRecord {
    /// A tab's *identity* for snapshot diffing.
    ///
    /// We key on the browser-assigned tab id when we have one, because that is
    /// stable across a tab's lifetime while its URL, title, pinned state and
    /// position all change. Without an id (session-file imports) we fall back to
    /// position + URL.
    pub fn identity(&self, tab_key: Option<&str>) -> String {
        match tab_key {
            Some(k) if !k.is_empty() => format!("{}\u{1}{}", self.window_id, k),
            _ => format!("{}\u{1}{}\u{1}{}", self.window_id, self.tab_index, self.url),
        }
    }
}

/// A window as seen at one instant.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowSnapshot {
    pub window_id: String,
    pub focused: bool,
    pub incognito: bool,
    pub tabs: Vec<TabRecord>,
}

/// One browser profile's entire visible state at one instant.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowserSnapshot {
    pub browser: String,
    pub profile: String,
    pub windows: Vec<WindowSnapshot>,
}

impl BrowserSnapshot {
    /// Flattens all tabs, tagging each with the window it belongs to.
    pub fn all_tabs(&self) -> impl Iterator<Item = (&WindowSnapshot, &TabRecord)> {
        self.windows
            .iter()
            .flat_map(|w| w.tabs.iter().map(move |t| (w, t)))
    }

    pub fn tab_count(&self) -> usize {
        self.windows.iter().map(|w| w.tabs.len()).sum()
    }
}

/// A stored snapshot row (without its window/tab payload, which lives in
/// child tables and is loaded on demand).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMeta {
    pub id: i64,
    pub taken_ms: i64,
    pub browser: String,
    pub profile: String,
    pub fingerprint: String,
    pub tab_count: i64,
    pub window_count: i64,
    pub reason: SnapshotReason,
    /// True when this snapshot was flagged as a recovery point (mass-close).
    pub recovery_point: bool,
}

/// Why a snapshot was taken. Drives UI labelling and retention (recovery points
/// are always kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    /// Periodic timer.
    Interval,
    /// A tab or window closed.
    TabClosed,
    /// A mass-close was detected.
    MassClose,
    /// Browser startup or extension install (first capture).
    Startup,
    /// Taken on demand from the UI.
    Manual,
    /// Imported from browser session files rather than observed live.
    Imported,
}

impl SnapshotReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            SnapshotReason::Interval => "interval",
            SnapshotReason::TabClosed => "tab_closed",
            SnapshotReason::MassClose => "mass_close",
            SnapshotReason::Startup => "startup",
            SnapshotReason::Manual => "manual",
            SnapshotReason::Imported => "imported",
        }
    }

    pub fn parse(s: &str) -> Option<SnapshotReason> {
        Some(match s {
            "interval" => SnapshotReason::Interval,
            "tab_closed" => SnapshotReason::TabClosed,
            "mass_close" => SnapshotReason::MassClose,
            "startup" => SnapshotReason::Startup,
            "manual" => SnapshotReason::Manual,
            "imported" => SnapshotReason::Imported,
            _ => return None,
        })
    }

    pub const fn is_recovery_point(self) -> bool {
        matches!(self, SnapshotReason::MassClose)
    }
}

/// A window that was closed, recorded so "restore last closed window" is a
/// single indexed lookup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosedWindowEvent {
    pub id: i64,
    pub browser: String,
    pub profile: String,
    pub window_id: String,
    pub closed_ms: i64,
    pub tab_count: i64,
    pub pinned_count: i64,
    pub tabs: Vec<TabRecord>,
}

/// Everything the extension can tell us about the browser it is running in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowserIdentity {
    /// `chrome` | `edge` | `firefox` — as reported by the extension.
    pub browser: String,
    /// Profile label (the extension passes the profile's display name).
    pub profile: String,
    /// Extension version, so we can warn about a stale host pairing.
    pub extension_version: String,
}

/// Settings persisted in the `settings` table. Serialized as JSON under a single
/// key so adding a field never needs a schema migration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Seconds between periodic snapshots.
    pub capture_interval_secs: u32,
    /// Record incognito/private windows.
    pub capture_incognito: bool,
    /// Domain suffixes never recorded.
    pub exclusions: Vec<String>,
    /// Keep every snapshot newer than this many hours.
    pub retention_keep_all_hours: u32,
    /// Beyond that, keep one per hour for this many days.
    pub retention_hourly_days: u32,
    /// Beyond that, keep one per day for this many days.
    pub retention_daily_days: u32,
    /// Never delete snapshots flagged as recovery points.
    pub keep_recovery_points_forever: bool,
    /// Start the app when the user logs in.
    pub start_with_windows: bool,
    /// Minimise to tray instead of exiting when the window is closed.
    pub minimise_to_tray: bool,
    /// `dark` | `light` | `system`.
    pub theme: String,
    /// Master pause switch for all recording.
    pub recording_paused: bool,
    /// Encrypt the database with DPAPI-protected SQLCipher key material.
    pub encryption_enabled: bool,
    /// Take a daily backup of the database.
    pub daily_backup: bool,
    /// Number of daily backups to keep.
    pub backup_keep: u32,
    /// Show a Windows toast when a mass-close is detected.
    pub notify_on_mass_close: bool,
    /// Number of tabs closed within `mass_close_window_secs` that triggers a
    /// recovery point.
    pub mass_close_tab_threshold: u32,
    /// The sliding window for mass-close detection.
    pub mass_close_window_secs: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            capture_interval_secs: 60,
            capture_incognito: false,
            exclusions: Vec::new(),
            retention_keep_all_hours: 24,
            retention_hourly_days: 30,
            retention_daily_days: 365,
            keep_recovery_points_forever: true,
            start_with_windows: false,
            minimise_to_tray: true,
            theme: "system".to_string(),
            recording_paused: false,
            encryption_enabled: false,
            daily_backup: true,
            backup_keep: 14,
            notify_on_mass_close: true,
            mass_close_tab_threshold: 5,
            mass_close_window_secs: 5,
        }
    }
}

impl Settings {
    /// Clamps user input into sane ranges so a typo cannot disable capture or
    /// make retention delete everything.
    pub fn sanitize(&mut self) {
        self.capture_interval_secs = self.capture_interval_secs.clamp(5, 3600);
        self.retention_keep_all_hours = self.retention_keep_all_hours.clamp(1, 24 * 30);
        self.retention_hourly_days = self.retention_hourly_days.clamp(1, 3650);
        self.retention_daily_days = self.retention_daily_days.clamp(1, 3650);
        self.backup_keep = self.backup_keep.clamp(1, 365);
        self.mass_close_tab_threshold = self.mass_close_tab_threshold.clamp(2, 500);
        self.mass_close_window_secs = self.mass_close_window_secs.clamp(1, 120);
        if !matches!(self.theme.as_str(), "dark" | "light" | "system") {
            self.theme = "system".to_string();
        }
        // De-duplicate and normalise exclusions.
        let mut seen = std::collections::BTreeSet::new();
        self.exclusions = self
            .exclusions
            .drain(..)
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| !e.is_empty())
            .filter(|e| seen.insert(e.clone()))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_and_confidence_roundtrip() {
        for s in [
            Source::Extension,
            Source::ChromiumSnss,
            Source::FirefoxSessionStore,
            Source::FirefoxPlaces,
            Source::ChromiumHistory,
            Source::Import,
        ] {
            assert_eq!(Source::parse(s.as_str()), Some(s));
        }
        for c in [Confidence::High, Confidence::Medium, Confidence::Low] {
            assert_eq!(Confidence::parse(c.as_str()), Some(c));
        }
        assert_ne!(Confidence::High, Confidence::Low);
    }

    #[test]
    fn snapshot_reason_recovery_flag() {
        assert!(SnapshotReason::MassClose.is_recovery_point());
        assert!(!SnapshotReason::Interval.is_recovery_point());
    }

    #[test]
    fn identity_prefers_tab_key() {
        let mut t = TabRecord {
            window_id: "7".into(),
            tab_index: 3,
            url: "https://a.com".into(),
            ..Default::default()
        };
        let by_key = t.identity(Some("42"));
        t.url = "https://b.com".into();
        assert_eq!(t.identity(Some("42")), by_key, "tab key must dominate URL");
        assert_ne!(t.identity(None), by_key);
    }

    #[test]
    fn settings_sanitize_clamps_and_dedupes() {
        let mut s = Settings {
            capture_interval_secs: 0,
            retention_keep_all_hours: 0,
            backup_keep: 99_999,
            mass_close_tab_threshold: 1,
            theme: "neon".into(),
            exclusions: vec![
                " Example.COM ".into(),
                "example.com".into(),
                "".into(),
                "  ".into(),
            ],
            ..Default::default()
        };
        s.sanitize();
        assert_eq!(s.capture_interval_secs, 5);
        assert_eq!(s.retention_keep_all_hours, 1);
        assert_eq!(s.backup_keep, 365);
        assert_eq!(s.mass_close_tab_threshold, 2);
        assert_eq!(s.theme, "system");
        assert_eq!(s.exclusions, vec!["example.com".to_string()]);
    }

    #[test]
    fn settings_roundtrip_through_json() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn settings_tolerate_unknown_and_missing_fields() {
        // Forward compatibility: an older app reading a newer config.
        let back: Settings =
            serde_json::from_str(r#"{"captureIntervalSecs":30,"futureField":true}"#).unwrap();
        assert_eq!(back.capture_interval_secs, 30);
        assert_eq!(back.retention_hourly_days, 30);
    }
}
