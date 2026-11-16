//! Wire protocol between the browser extension and the native host.
//!
//! Both directions are JSON messages, one per native-messaging frame
//! (`uint32` LE length prefix + UTF-8 JSON). Field names are **camelCase**,
//! matching JavaScript conventions; Rust field identifiers stay snake_case via
//! `rename_all = "camelCase"`, so the two sides can never drift.

use serde::{Deserialize, Serialize};
use tv_core::model::BrowserSnapshot;

/// The subset of vault settings pushed to the extension on every poll reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveAppSettings {
    pub paused: bool,
    pub capture_interval_secs: u32,
    pub capture_incognito: bool,
    pub notify_on_mass_close: bool,
    pub mass_close_tab_threshold: u32,
    pub mass_close_window_secs: u32,
}

/// Messages the extension can send to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ExtensionMessage {
    /// Extension identifies itself on connect.
    Hello {
        browser: String,
        profile: String,
        #[serde(default)]
        extension_version: String,
    },
    /// Full browser state (filtered of nothing — the host does that).
    ///
    /// `reason` is one of: `interval` (periodic), `tab_closed`, `startup`,
    /// `manual`, `mass_close`.
    State {
        snapshot: BrowserSnapshot,
        reason: String,
    },
    /// The extension asks for pending restore jobs.
    PollRestore {
        /// Cap on jobs returned in one poll; the extension drains over multiple polls.
        #[serde(default = "default_poll_limit")]
        limit: u32,
    },
    /// A restore job completed (or failed) — reports progress to the UI.
    RestoreDone {
        job_id: i64,
        #[serde(default)]
        error: Option<String>,
    },
    /// Pause/resume recording from the extension's popup/options page.
    SetPaused {
        paused: bool,
    },
    /// Entries from the browser's sessions API (recently closed tabs).
    ImportSessions {
        entries: Vec<tv_core::db::write::ImportSessionEntry>,
    },
    /// Aggregated visit counts from the browser's history (seeded stats).
    ImportHistory {
        entries: Vec<tv_core::db::write::ImportHistoryEntry>,
    },
    /// Health check; the host replies with a pong.
    Ping,
    /// Launch the desktop app (notification click or options page button).
    OpenApp,
}

fn default_poll_limit() -> u32 {
    10
}

/// Messages the host can send to the extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HostMessage {
    /// Sent in reply to `hello`.
    HelloAck {
        app_version: String,
        /// Capabilities the extension can rely on.
        capabilities: Vec<String>,
    },
    /// Acknowledgement of a `state` message.
    Ack {
        /// The snapshot id, when one was created.
        snapshot_id: Option<i64>,
        /// True when nothing changed and no row was written.
        unchanged: bool,
        /// True when this ingest was flagged as a recovery point.
        #[serde(default)]
        recovery_point: bool,
    },
    /// Pending restore jobs for the extension to execute.
    Restore {
        jobs: Vec<RestoreJob>,
        /// Live settings from the vault (authoritative copy), pushed so the
        /// extension's popup/options pages always show the truth.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settings: Option<LiveAppSettings>,
    },
    /// Acknowledgement of a `set_paused` request, with the applied value.
    AckSettings {
        settings: LiveAppSettings,
    },
    /// Asks the extension to show a Windows toast / notification.
    Notify {
        title: String,
        body: String,
        /// Correlates with the snapshot that triggered it.
        tag: String,
    },
    /// Response to `ping`.
    Pong {
        app_version: String,
    },
    /// A failure that prevented the requested action.
    HostError {
        /// Machine-readable code.
        code: String,
        /// Human-readable explanation (shown in the extension's error log).
        message: String,
    },
}

/// A restore job for the extension to execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreJob {
    pub id: i64,
    pub action: String,
    /// Opaque JSON payload; the extension interprets it.
    pub payload: serde_json::Value,
}

impl HostMessage {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        HostMessage::HostError {
            code: code.into(),
            message: message.into(),
        }
    }
}
