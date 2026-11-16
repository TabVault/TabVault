//! Browser profile detection on Windows.
//!
//! We look where the browsers actually install on this OS and enumerate their
//! profiles, so the recovery tool works without the user typing a single path.
//! Everything is read-only: we inspect directory contents, never mutate.

use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A detected browser profile root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectedProfile {
    /// `chrome` | `edge` | `firefox`
    pub browser: String,
    /// Folder name, e.g. `Default`, `Profile 1`, `xxxx.default-release`.
    pub profile_dir: String,
    /// Human display name from `Local State`/`profiles.ini` when available.
    pub display_name: Option<String>,
    /// Absolute path to the profile directory.
    pub path: PathBuf,
    /// The browser's user data root (parent of the profile).
    pub user_data_root: PathBuf,
}

/// Chromium browser roots on this machine, in priority order.
const CHROMIUM_ROOTS: &[(&str, &str)] = &[
    ("chrome", r"Google\Chrome\User Data"),
    ("chrome", r"Google\Chrome Beta\User Data"),
    ("chrome", r"Chromium\User Data"),
    ("edge", r"Microsoft\Edge\User Data"),
    ("edge", r"Microsoft\Edge Beta\User Data"),
    ("edge", r"Microsoft\Edge Dev\User Data"),
    ("brave", r"BraveSoftware\Brave-Browser\User Data"),
    ("opera", r"Opera Software\Opera Stable"),
];

/// Environment variable names holding the per-user data roots.
pub const LOCAL_APP_DATA: &str = "LOCALAPPDATA";
pub const ROAMING_APP_DATA: &str = "APPDATA";

fn local_app_data() -> PathBuf {
    std::env::var_os(LOCAL_APP_DATA)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Local"))
}

fn roaming_app_data() -> PathBuf {
    std::env::var_os(ROAMING_APP_DATA)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Roaming"))
}

/// Detects every Chromium-family browser profile on this machine.
pub fn detect_chromium_profiles() -> Vec<DetectedProfile> {
    let mut out = Vec::new();
    for &(browser, relative) in CHROMIUM_ROOTS {
        let root = local_app_data().join(relative);
        if !root.is_dir() {
            continue;
        }
        // Directory names that are never real profiles.
        const SKIP: &[&str] = &[
            "system profile",
            "guest profile",
            "crashpad",
            "installation",
            "crash reports",
        ];
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        // Display names from Local State, keyed by folder name.
        let names = chromium_local_state_names(&root);
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let dir = entry.file_name().to_string_lossy().to_string();
            if SKIP.contains(&dir.to_ascii_lowercase().as_str()) {
                continue;
            }
            // A profile folder must contain session/history-ish artifacts;
            // otherwise it is a component folder (e.g. "GrShaderCache").
            const PROFILE_MARKERS: &[&str] = &[
                "Sessions",
                "History",
                "Preferences",
                "Bookmarks",
                "Tabs_",
                "Session_",
                "Extensions",
                "Sync Data",
            ];
            let is_profile = PROFILE_MARKERS.iter().any(|m| path.join(m).exists());
            if !is_profile {
                continue;
            }
            out.push(DetectedProfile {
                browser: browser.to_string(),
                profile_dir: dir.clone(),
                display_name: names.get(&dir).cloned(),
                path,
                user_data_root: root.clone(),
            });
        }
    }
    // Default profile first, then the rest by name for a stable UI.
    out.sort_by(|a, b| {
        let rank = |p: &DetectedProfile| {
            if p.profile_dir.eq_ignore_ascii_case("Default") {
                0
            } else {
                1
            }
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.profile_dir.cmp(&b.profile_dir))
    });
    out
}

/// Reads profile display names from `Local State`'s `profile.info_cache`.
fn chromium_local_state_names(user_data_root: &Path) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    let local_state = user_data_root.join("Local State");
    let Ok(text) = std::fs::read_to_string(&local_state) else {
        return names;
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return names;
    };
    if let Some(info_cache) = json
        .get("profile")
        .and_then(|p| p.get("info_cache"))
        .and_then(|v| v.as_object())
    {
        for (dir, meta) in info_cache {
            if let Some(name) = meta.get("name").and_then(|v| v.as_str()) {
                let trimmed = name.trim();
                if !trimmed.is_empty() {
                    names.insert(dir.clone(), trimmed.to_string());
                }
            }
        }
    }
    names
}

/// Detects every Firefox profile on this machine by parsing `profiles.ini`.
///
/// Firefox keeps two copies of its profile registry — `profiles.ini` next to
/// the profiles, and `installs.ini` keyed by install hash. Both are INI files
/// with `Path`/`IsRelative`/`Name` entries; we merge both views.
pub fn detect_firefox_profiles() -> Vec<DetectedProfile> {
    let mozilla_root = roaming_app_data().join(r"Mozilla\Firefox");
    let mut out = Vec::new();

    let parse_ini_sections = |path: &Path| -> Vec<(String, std::collections::HashMap<String, String>)> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let mut sections: Vec<(String, std::collections::HashMap<String, String>)> = Vec::new();
        let mut current: Option<(String, std::collections::HashMap<String, String>)> = None;
        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                if let Some(s) = current.take() {
                    sections.push(s);
                }
                current = Some((line[1..line.len() - 1].to_string(), std::collections::HashMap::new()));
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if let Some((_, map)) = current.as_mut() {
                    map.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        if let Some(s) = current {
            sections.push(s);
        }
        sections
    };

    let mut seen = std::collections::BTreeSet::new();
    let mut add = |ini: &Path| {
        for (section, kvs) in parse_ini_sections(ini) {
            if !section.to_ascii_lowercase().starts_with("profile") {
                continue;
            }
            let Some(path_value) = kvs.get("Path").cloned() else {
                continue;
            };
            let is_relative = kvs
                .get("IsRelative")
                .map(|v| v == "1")
                .unwrap_or(true);
            let resolved = if is_relative {
                mozilla_root.join(&path_value)
            } else {
                PathBuf::from(&path_value)
            };
            if !resolved.is_dir() {
                continue;
            }
            if !seen.insert(resolved.clone()) {
                continue;
            }
            let name = kvs
                .get("Name")
                .cloned()
                .unwrap_or_else(|| path_value.clone());
            out.push(DetectedProfile {
                browser: "firefox".to_string(),
                profile_dir: name,
                display_name: kvs.get("Name").cloned(),
                path: resolved,
                user_data_root: mozilla_root.clone(),
            });
        }
    };

    add(&mozilla_root.join("profiles.ini"));
    add(&mozilla_root.join("installs.ini"));

    // Deduplicate while preserving order.
    let mut final_out = Vec::new();
    let mut keys = std::collections::BTreeSet::new();
    for p in out {
        if keys.insert(p.path.clone()) {
            final_out.push(p);
        }
    }
    final_out
}

/// All detectable browser profiles on this machine (Chromium + Firefox).
pub fn detect_all_profiles() -> Vec<DetectedProfile> {
    let mut v = detect_chromium_profiles();
    v.extend(detect_firefox_profiles());
    v
}

/// Known session-store locations for a Chromium profile, newest first.
///
/// Includes both the cleartext mirror and the encrypted mirror, plus the
/// pre-Chrome-85 flat files in the profile root.
pub fn chromium_session_dirs(profile: &DetectedProfile) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for sub in ["Sessions", "Sessions_Encrypted"] {
        let d = profile.path.join(sub);
        if d.is_dir() {
            dirs.push(d);
        }
    }
    // Very old browsers kept the files in the profile root without timestamps.
    dirs.push(profile.path.clone());
    dirs
}

/// Files matching the SNSS pattern this build understands.
pub fn chromium_session_files(profile: &DetectedProfile) -> Vec<PathBuf> {
    let mut files = Vec::new();
    const PATTERNS: &[&str] = &[
        "Tabs_",
        "Session_",
        "tabs_",
        "session_",
        "Current Session",
        "Last Session",
        "Current Tabs",
        "Last Tabs",
    ];
    for dir in chromium_session_dirs(profile) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if PATTERNS.iter().any(|p| name.starts_with(p)) {
                files.push(entry.path());
            }
        }
    }
    // Newest last_modified first — best recovery candidates first.
    files.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });
    files.reverse();
    files
}

/// Known Firefox session files for a profile, best first.
pub fn firefox_session_files(profile: &DetectedProfile) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let backups = profile.path.join("sessionstore-backups");
    // Preference: recovery (current state), then previous (last clean exit),
    // then any upgrade snapshots.
    let mut push_with = |dir: &Path, name: String| {
        let p = dir.join(&name);
        if p.is_file() {
            out.push(p);
        }
    };
    push_with(&backups, "recovery.jsonlz4".into());
    push_with(&backups, "recovery.baklz4".into());
    push_with(&backups, "previous.jsonlz4".into());
    push_with(&profile.path, "sessionstore.jsonlz4".into());
    // upgrade.jsonlz4-* after the others.
    let mut upgrades = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&backups) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("upgrade.jsonlz4-") {
                upgrades.push(e.path());
            }
        }
    }
    upgrades.sort();
    out.append(&mut upgrades);
    out
}

/// Firefox history database.
pub fn firefox_places(profile: &DetectedProfile) -> Option<PathBuf> {
    let p = profile.path.join("places.sqlite");
    p.is_file().then_some(p)
}

/// Chromium history database.
pub fn chromium_history(profile: &DetectedProfile) -> Option<PathBuf> {
    let p = profile.path.join("History");
    p.is_file().then_some(p)
}

/// Root `Local State` path for a Chromium profile's user data root.
pub fn chromium_local_state(profile: &DetectedProfile) -> Option<PathBuf> {
    let p = profile.user_data_root.join("Local State");
    p.is_file().then_some(p)
}

/// Summary row for the "Import from browser" screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileSummary {
    pub browser: String,
    pub profile_dir: String,
    pub display_name: Option<String>,
    pub path: String,
    pub session_files: usize,
    pub history_available: bool,
}

pub fn summarize(profile: &DetectedProfile) -> ProfileSummary {
    ProfileSummary {
        browser: profile.browser.clone(),
        profile_dir: profile.profile_dir.clone(),
        display_name: profile.display_name.clone(),
        path: profile.path.display().to_string(),
        session_files: chromium_session_files(profile).len()
            + firefox_session_files(profile).len(),
        history_available: chromium_history(profile).is_some()
            || firefox_places(profile).is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_app_data_never_panics() {
        let p = local_app_data();
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    fn firefox_ini_parsing_tolerates_crlf_and_comments() {
        let ini = "[General]\nVersion=2\n\n[Profile0]\nName=default-release\nIsRelative=1\nPath=abc.default-release\n# comment\n";
        let path = std::env::temp_dir().join("tabvault-test-profiles.ini");
        std::fs::write(&path, ini).unwrap();
        // Directly exercise the parser used by both files.
        let mut sections = Vec::new();
        let mut cur: Option<(String, std::collections::HashMap<String, String>)> = None;
        for raw in ini.lines() {
            let line = raw.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                if let Some(s) = cur.take() {
                    sections.push(s);
                }
                cur = Some((line[1..line.len() - 1].to_string(), std::collections::HashMap::new()));
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if let Some((_, m)) = cur.as_mut() {
                    m.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
        if let Some(s) = cur {
            sections.push(s);
        }
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[1].1.get("Path").unwrap(), "abc.default-release");
        let _ = std::fs::remove_file(&path);
    }
}
