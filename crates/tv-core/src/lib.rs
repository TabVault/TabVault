//! TabVault core.
//!
//! This crate is shared by two very different front ends that must never drift
//! apart:
//!
//! * `tabvault-host` — the native-messaging host the browser extension talks to.
//! * `tabvault` — the Tauri desktop app that owns the UI and the tray.
//!
//! Everything either of them must agree on lives here: the SQLite schema and its
//! migrations, snapshot fingerprinting/diffing, retention pruning, and the
//! browser session-file parsers.

pub mod crypto;
pub mod db;
pub mod error;
pub mod model;
pub mod parsers;
pub mod paths;
pub mod retention;
pub mod snapshot;
pub mod time;
pub mod url_util;
pub mod locked_copy;

pub use error::{Error, Result};
