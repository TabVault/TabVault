//! Wire protocol between the browser extension and the native host.
//!
//! Both directions are JSON messages, one per native-messaging frame
//! (`uint32` LE length prefix + UTF-8 JSON). Keeping the framing in one place
//! and the message shapes in `serde` structs guarantees the two sides never
//! disagree.

use serde::{Deserialize, Serialize};
use tv_core::model::BrowserSnapshot;

/// Messages the extension can send to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    /// A restore job completed (or failed) — the payload reports it to the UI.
    RestoreDone {
        job_id: i64,
        #[serde(default)]
        error: Option<String>,
    },
    /// Health check; the host replies with a pong.
    Ping,
}

fn default_poll_limit() -> u32 {
    10
}

/// Messages the host can send to the extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
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
        /// Number of recovery points retained in the vault after this ingest.
        #[serde(default)]
        recovery_point: bool,
    },
    /// Pending restore jobs for the extension to execute.
    Restore {
        jobs: Vec<RestoreJob>,
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
