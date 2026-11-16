//! Browser session-file parsers.
//!
//! - [`pickle`]: Chromium's `base::Pickle` reader (bounds-checked).
//! - [`snss`]: Chromium SNSS session/tab-restore files.
//! - [`firefox`]: Firefox sessionstore (mozlz4) and `places.sqlite`.
//! - [`chromium_history`]: Chromium `History` database (copied, then read).

pub mod chromium_history;
pub mod firefox;
pub mod pickle;
pub mod snss;
