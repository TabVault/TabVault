//! Snapshot fingerprinting and diffing.
//!
//! A periodic snapshot is only worth storing when something actually changed, so
//! an idle browser writes nothing. We compute a stable fingerprint over the
//! *meaningful* state of every tab and window, and only insert a new snapshot row
//! when the fingerprint differs from the profile's most recent one.
//!
//! "Meaningful" is the important word. We fingerprint fields the user would
//! notice losing — URLs, titles, pinned state, group membership and order, window
//! focus — and deliberately ignore volatile noise such as a favicon that was
//! still loading or a title that flipped from "Loading…" to the real one, which
//! would otherwise produce a new snapshot every single cycle.

use crate::model::{BrowserSnapshot, TabRecord, WindowSnapshot};
use crate::url_util;
use blake3::Hasher;

/// Fields of a tab that participate in change detection, in a fixed order.
///
/// Kept as an explicit list (rather than hashing the struct) so that adding a
/// field to `TabRecord` is a conscious decision about whether it should make
/// snapshots differ.
fn hash_tab(h: &mut Hasher, t: &TabRecord) {
    h.update(t.url.as_bytes());
    h.update(&[0]);
    h.update(t.title.as_bytes());
    h.update(&[0]);
    h.update(&[t.pinned as u8]);
    h.update(&t.tab_index.to_le_bytes());
    h.update(&[0]);
    h.update(t.group_name.as_deref().unwrap_or("").as_bytes());
    h.update(&[0]);
    h.update(t.group_color.as_deref().unwrap_or("").as_bytes());
    // `active_ms`/`last_active_ms` change constantly while the user works and
    // must NOT make a snapshot differ, or the dedupe never fires. They are
    // updated in place on the existing snapshot row instead.
}

fn hash_window(h: &mut Hasher, w: &WindowSnapshot) {
    h.update(w.window_id.as_bytes());
    h.update(&[0]);
    h.update(&[w.focused as u8]);
    // Order matters: two windows with the same tabs in a different order look
    // different to the user, so we hash tabs in visual order.
    let mut ordered: Vec<&TabRecord> = w.tabs.iter().collect();
    ordered.sort_by_key(|t| t.tab_index);
    h.update(&(ordered.len() as u64).to_le_bytes());
    for t in ordered {
        hash_tab(h, t);
        h.update(&[0xFF]);
    }
}

/// A stable hex fingerprint of a profile's snapshot.
///
/// Windows are sorted by id so that the order the browser happened to report
/// them in does not create a spurious diff.
pub fn fingerprint(snap: &BrowserSnapshot) -> String {
    let mut h = Hasher::new();
    h.update(b"tabvault-snapshot-v1");
    h.update(snap.browser.as_bytes());
    h.update(&[0]);
    h.update(snap.profile.as_bytes());
    h.update(&[0]);

    let mut windows: Vec<&WindowSnapshot> = snap.windows.iter().collect();
    windows.sort_by(|a, b| a.window_id.cmp(&b.window_id));
    h.update(&(windows.len() as u64).to_le_bytes());
    for w in windows {
        hash_window(&mut h, w);
        h.update(&[0xFE]);
    }
    h.finalize().to_hex().to_string()
}

/// What changed between two snapshots. Used for the UI diff view and to decide
/// whether a "tab closed" snapshot is worth keeping.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SnapshotDiff {
    pub added: Vec<TabRecord>,
    pub removed: Vec<TabRecord>,
    pub changed: Vec<TabChange>,
    pub windows_added: Vec<String>,
    pub windows_removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabChange {
    pub before: TabRecord,
    pub after: TabRecord,
    /// Which fields changed, as stable identifiers (`pinned`, `url`, `title`,
    /// `group`, `index`).
    pub fields: Vec<&'static str>,
}

impl SnapshotDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.windows_added.is_empty()
            && self.windows_removed.is_empty()
    }

    /// Number of tabs present after the change.
    pub fn tab_delta(&self) -> i64 {
        self.added.len() as i64 - self.removed.len() as i64
    }
}

/// Flattens a snapshot into `(identity -> tab)`.
///
/// Identity prefers the browser's own tab id (stable across the tab's lifetime,
/// across reorders and navigations). Session-file imports have no tab id, so
/// they fall back to position + URL, which is the best a file can offer.
fn index_tabs(snap: &BrowserSnapshot) -> std::collections::BTreeMap<String, TabRecord> {
    let mut map = std::collections::BTreeMap::new();
    for (w, t) in snap.all_tabs() {
        let key = match &t.browser_tab_id {
            Some(id) if !id.is_empty() => format!("{}\u{1}tab:{}", w.window_id, id),
            _ => format!("{}\u{1}{}\u{1}{}", w.window_id, t.tab_index, t.url),
        };
        map.insert(key, t.clone());
    }
    map
}

/// Computes the difference between `before` and `after`.
pub fn diff(before: &BrowserSnapshot, after: &BrowserSnapshot) -> SnapshotDiff {
    let b = index_tabs(before);
    let a = index_tabs(after);
    let mut out = SnapshotDiff::default();

    for (key, tab) in &a {
        match b.get(key) {
            None => out.added.push(tab.clone()),
            Some(old) => {
                let mut fields = Vec::new();
                if old.url != tab.url {
                    fields.push("url");
                }
                if old.title != tab.title {
                    fields.push("title");
                }
                if old.pinned != tab.pinned {
                    fields.push("pinned");
                }
                if old.group_name != tab.group_name || old.group_color != tab.group_color {
                    fields.push("group");
                }
                if !fields.is_empty() {
                    out.changed.push(TabChange {
                        before: old.clone(),
                        after: tab.clone(),
                        fields,
                    });
                }
            }
        }
    }
    for (key, tab) in &b {
        if !a.contains_key(key) {
            out.removed.push(tab.clone());
        }
    }

    let wb: std::collections::BTreeSet<&str> =
        before.windows.iter().map(|w| w.window_id.as_str()).collect();
    let wa: std::collections::BTreeSet<&str> =
        after.windows.iter().map(|w| w.window_id.as_str()).collect();
    for w in wa.difference(&wb) {
        out.windows_added.push((*w).to_string());
    }
    for w in wb.difference(&wa) {
        out.windows_removed.push((*w).to_string());
    }
    out
}

// ---------------------------------------------------------------------------
// Window migration detection
// ---------------------------------------------------------------------------

/// A "window signature" for migration detection: the ordered multiset of
/// (url, pinned, index, group) per tab. Two windows with the same signature
/// hold the same content, even if the browser rebuilt them under a new id.
fn window_signature(w: &crate::model::WindowSnapshot) -> Vec<(String, bool, i64, String, String)> {
    let mut sig: Vec<(String, bool, i64, String, String)> = w
        .tabs
        .iter()
        .map(|t| {
            (
                t.url.clone(),
                t.pinned,
                t.tab_index,
                t.group_name.clone().unwrap_or_default(),
                t.group_color.clone().unwrap_or_default(),
            )
        })
        .collect();
    sig.sort_by_key(|(_, _, idx, _, _)| *idx);
    sig
}

/// Detects when every window that vanished reappeared unchanged in a new id —
/// i.e. the browser rebuilt windows (startup restore, session restore), not a
/// user-facing mass close. Only then are close events suppressed.
pub fn detect_window_migration(
    before: &BrowserSnapshot,
    after: &BrowserSnapshot,
    diff: &SnapshotDiff,
) -> bool {
    if diff.windows_removed.is_empty() || diff.windows_added.is_empty() {
        return false;
    }
    let added_sigs: Vec<Vec<(String, bool, i64, String, String)>> = after
        .windows
        .iter()
        .filter(|w| diff.windows_added.iter().any(|id| id == &w.window_id))
        .map(window_signature)
        .collect();
    let removed_sigs: Vec<Vec<(String, bool, i64, String, String)>> = before
        .windows
        .iter()
        .filter(|w| diff.windows_removed.iter().any(|id| id == &w.window_id))
        .map(window_signature)
        .collect();
    if removed_sigs.is_empty() || removed_sigs.len() != added_sigs.len() {
        return false;
    }
    // Every removed window must find exactly one unused added window with an
    // identical signature.
    let mut used = vec![false; added_sigs.len()];
    'outer: for removed in &removed_sigs {
        for (i, added) in added_sigs.iter().enumerate() {
            if !used[i] && added == removed {
                used[i] = true;
                continue 'outer;
            }
        }
        return false;
    }
    true
}

/// True when `n` tabs closed within the configured window should be treated as
/// a mass-close recovery point.
pub fn is_mass_close(n_events: usize, threshold: u32) -> bool {
    n_events as u64 >= threshold as u64
}

/// (Retained for unit tests of the sliding-window math; the ingest path uses
/// a SQL count against `closed_tab_events` instead.)
#[cfg(test)]
pub fn count_recent(events: &[i64], now_ms: i64, window_secs: u32) -> usize {
    let cutoff = now_ms - (window_secs as i64) * 1000;
    events
        .iter()
        .rev()
        .take_while(|&&t| t >= cutoff)
        .count()
}

/// Normalises a snapshot before storage: drops excluded and (optionally)
/// incognito tabs, renumbers indices, and strips any URL that is not worth
/// restoring.
///
/// Returns the snapshot plus the number of tabs that were filtered out, so the
/// UI can explain "12 tabs hidden by your exclusion list".
pub fn filter_snapshot(
    mut snap: BrowserSnapshot,
    exclusions: &[String],
    capture_incognito: bool,
) -> (BrowserSnapshot, usize) {
    let mut dropped = 0usize;
    snap.windows.retain_mut(|w| {
        if w.incognito && !capture_incognito {
            dropped += w.tabs.len();
            return false;
        }
        let before = w.tabs.len();
        w.tabs.retain(|t| {
            if t.incognito && !capture_incognito {
                return false;
            }
            !should_skip_url(&t.url, exclusions)
        });
        dropped += before - w.tabs.len();
        // Re-number so a filtered list still has contiguous indices.
        let mut ordered = std::mem::take(&mut w.tabs);
        ordered.sort_by_key(|t| t.tab_index);
        for (i, t) in ordered.iter_mut().enumerate() {
            t.tab_index = i as i64;
            // Pinned tabs always come first in a real browser window.
        }
        w.tabs = ordered;
        if w.tabs.is_empty() {
            // Keep the window record so "a window with only excluded tabs" is
            // still visible in the timeline as an empty window.
            return true;
        }
        true
    });
    (snap, dropped)
}

/// URLs that can never be restored and therefore should not be stored.
pub fn should_skip_url(url: &str, exclusions: &[String]) -> bool {
    if url.is_empty() {
        return true;
    }
    let lower = url.trim().to_ascii_lowercase();
    if lower == "about:blank"
        || lower.starts_with("chrome://newtab")
        || lower.starts_with("edge://newtab")
        || lower.starts_with("devtools://")
        || lower.starts_with("chrome-extension://")
        || lower.starts_with("moz-extension://")
        || lower.starts_with("about:newtab")
        || lower.starts_with("about:home")
    {
        return true;
    }
    if url_util::is_excluded(url, exclusions) {
        return true;
    }
    // Never store a URL that embeds credentials.
    if let Some(after_scheme) = url.split_once("://").map(|(_, r)| r) {
        if let Some((userinfo, _)) = after_scheme.split_once('@') {
            if userinfo.contains(':') {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;

    fn tab(url: &str, index: i64, pinned: bool) -> TabRecord {
        TabRecord {
            url: url.to_string(),
            title: format!("title of {url}"),
            tab_index: index,
            pinned,
            window_id: "1".into(),
            source: Source::Extension,
            ..Default::default()
        }
    }

    fn snap(tabs: Vec<TabRecord>) -> BrowserSnapshot {
        BrowserSnapshot {
            browser: "chrome".into(),
            profile: "Default".into(),
            windows: vec![WindowSnapshot {
                window_id: "1".into(),
                focused: true,
                incognito: false,
                tabs,
            }],
        }
    }

    #[test]
    fn identical_snapshots_have_equal_fingerprints() {
        let a = snap(vec![tab("https://a.com", 0, false), tab("https://b.com", 1, true)]);
        let b = a.clone();
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn window_reporting_order_does_not_change_the_fingerprint() {
        let mut a = snap(vec![tab("https://a.com", 0, false)]);
        a.windows.push(WindowSnapshot {
            window_id: "2".into(),
            focused: false,
            incognito: false,
            tabs: vec![tab("https://z.com", 0, false)],
        });
        let mut b = a.clone();
        b.windows.reverse();
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn tab_order_within_a_window_does_change_the_fingerprint() {
        let a = snap(vec![tab("https://a.com", 0, false), tab("https://b.com", 1, false)]);
        let b = snap(vec![tab("https://b.com", 0, false), tab("https://a.com", 1, false)]);
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn volatile_fields_do_not_change_the_fingerprint() {
        // The whole point of dedupe: an idle browser with a ticking clock must
        // not write a snapshot every cycle.
        let mut a = snap(vec![tab("https://a.com", 0, false)]);
        let mut b = a.clone();
        b.windows[0].tabs[0].active_ms = 999_999;
        b.windows[0].tabs[0].last_active_ms = Some(123);
        b.windows[0].tabs[0].favicon = Some("https://a.com/f.ico".into());
        b.windows[0].tabs[0].visit_count = 77;
        assert_eq!(fingerprint(&a), fingerprint(&b));
        a.windows[0].tabs[0].active_ms = 1;
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn meaningful_changes_do_change_the_fingerprint() {
        let base = snap(vec![tab("https://a.com", 0, false)]);
        let mut pinned = base.clone();
        pinned.windows[0].tabs[0].pinned = true;
        assert_ne!(fingerprint(&base), fingerprint(&pinned));

        let mut retitled = base.clone();
        retitled.windows[0].tabs[0].title = "new".into();
        assert_ne!(fingerprint(&base), fingerprint(&retitled));

        let mut grouped = base.clone();
        grouped.windows[0].tabs[0].group_name = Some("Work".into());
        assert_ne!(fingerprint(&base), fingerprint(&grouped));
    }

    #[test]
    fn diff_reports_added_removed_changed() {
        let before = snap(vec![tab("https://a.com", 0, false), tab("https://b.com", 1, false)]);
        let mut after = snap(vec![tab("https://a.com", 0, false), tab("https://c.com", 1, false)]);
        after.windows[0].tabs[0].pinned = true;

        let d = diff(&before, &after);
        assert_eq!(d.added.len(), 1);
        assert_eq!(d.added[0].url, "https://c.com");
        assert_eq!(d.removed.len(), 1);
        assert_eq!(d.removed[0].url, "https://b.com");
        assert_eq!(d.changed.len(), 1);
        assert_eq!(d.changed[0].fields, vec!["pinned"]);
        assert_eq!(d.tab_delta(), 0);
        assert!(!d.is_empty());
    }

    #[test]
    fn diff_detects_window_appearance_and_disappearance() {
        let before = snap(vec![tab("https://a.com", 0, false)]);
        let mut after = before.clone();
        after.windows[0].window_id = "9".into();
        let d = diff(&before, &after);
        assert_eq!(d.windows_added, vec!["9".to_string()]);
        assert_eq!(d.windows_removed, vec!["1".to_string()]);
    }

    #[test]
    fn identical_window_recreated_under_new_id_is_a_migration_not_a_close() {
        let tab = |url: &str, index: i64| TabRecord {
            url: url.to_string(),
            title: url.to_string(),
            tab_index: index,
            pinned: false,
            window_id: "7".to_string(),
            source: crate::model::Source::Extension,
            ..Default::default()
        };
        let mut before = snap(vec![tab("https://a.com", 0), tab("https://b.com", 1)]);
        before.windows[0].window_id = "7".to_string();
        let mut after = snap(vec![tab("https://a.com", 0), tab("https://b.com", 1)]);
        after.windows[0].window_id = "9".to_string();

        let d = diff(&before, &after);
        assert_eq!(d.windows_removed, vec!["7".to_string()]);
        assert_eq!(d.windows_added, vec!["9".to_string()]);
        assert!(crate::snapshot::detect_window_migration(&before, &after, &d));
    }

    #[test]
    fn partial_window_changes_are_not_a_migration() {
        let tab = |url: &str, index: i64, id: &str| TabRecord {
            url: url.to_string(),
            title: url.to_string(),
            tab_index: index,
            window_id: id.to_string(),
            source: crate::model::Source::Extension,
            ..Default::default()
        };
        let before = BrowserSnapshot {
            browser: "chrome".into(),
            profile: "Default".into(),
            windows: vec![WindowSnapshot {
                window_id: "7".into(),
                focused: true,
                incognito: false,
                tabs: vec![tab("https://a.com", 0, "7"), tab("https://b.com", 1, "7")],
            }],
        };
        let after = BrowserSnapshot {
            browser: "chrome".into(),
            profile: "Default".into(),
            windows: vec![WindowSnapshot {
                window_id: "9".into(),
                focused: true,
                incognito: false,
                tabs: vec![tab("https://a.com", 0, "9"), tab("https://c.com", 1, "9")],
            }],
        };
        let d = diff(&before, &after);
        assert!(!crate::snapshot::detect_window_migration(&before, &after, &d));
    }

    #[test]
    fn no_window_change_means_no_migration() {
        let a = snap(vec![tab("https://a.com", 0, false)]);
        let d = diff(&a, &a);
        assert!(!crate::snapshot::detect_window_migration(&a, &a, &d));
    }

    #[test]
    fn empty_diff_for_identical_snapshots() {
        let a = snap(vec![tab("https://a.com", 0, false)]);
        assert!(diff(&a, &a).is_empty());
    }

    #[test]
    fn mass_close_threshold() {
        assert!(!is_mass_close(4, 5));
        assert!(is_mass_close(5, 5));
        assert!(is_mass_close(22, 5));
    }

    #[test]
    fn count_recent_respects_the_sliding_window() {
        let now = 1_000_000i64;
        let events = vec![now - 20_000, now - 4_000, now - 1_000, now];
        assert_eq!(count_recent(&events, now, 5), 3);
        assert_eq!(count_recent(&events, now, 1), 2);
        assert_eq!(count_recent(&events, now, 60), 4);
        assert_eq!(count_recent(&[], now, 5), 0);
    }

    #[test]
    fn filter_drops_excluded_and_incognito_and_renumbers() {
        let mut s = snap(vec![
            tab("https://keep.com", 0, false),
            tab("https://tracker.com", 1, false),
            tab("chrome://newtab", 2, false),
        ]);
        s.windows.push(WindowSnapshot {
            window_id: "2".into(),
            focused: false,
            incognito: true,
            tabs: vec![tab("https://private.com", 0, false)],
        });
        let (out, dropped) = filter_snapshot(s, &["tracker.com".into()], false);
        assert_eq!(out.windows.len(), 1);
        assert_eq!(out.windows[0].tabs.len(), 1);
        assert_eq!(out.windows[0].tabs[0].url, "https://keep.com");
        assert_eq!(out.windows[0].tabs[0].tab_index, 0);
        assert_eq!(dropped, 3);
    }

    #[test]
    fn filter_keeps_incognito_when_enabled() {
        let mut s = snap(vec![tab("https://keep.com", 0, false)]);
        s.windows.push(WindowSnapshot {
            window_id: "2".into(),
            focused: false,
            incognito: true,
            tabs: vec![tab("https://private.com", 0, false)],
        });
        let (out, dropped) = filter_snapshot(s, &[], true);
        assert_eq!(out.windows.len(), 2);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn credential_urls_are_never_stored() {
        assert!(should_skip_url("https://user:pass@example.com/x", &[]));
        assert!(!should_skip_url("https://example.com/x", &[]));
        assert!(should_skip_url("", &[]));
    }
}
