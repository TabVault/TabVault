//! `tabvault-host` — Chrome/Edge/Firefox native-messaging host for TabVault.
//!
//! The browser spawns this process and talks to it over stdin/stdout using the
//! native-messaging framing: a `uint32` native-byte-order message length, then
//! that many bytes of UTF-8 JSON. We never print anything but protocol frames
//! on stdout; diagnostics go to stderr (visible in the browser's error log with
//! `--enable-logging`).
//!
//! The host is intentionally stateless except for the database: everything it
//! needs to know about the world it rebuilds from the `live_state` table, so a
//! restart caused by a browser shutdown loses nothing.
//!
//! Usage:
//!   tabvault-host               # run the protocol loop (the normal mode)
//!   tabvault-host --selftest    # open the vault, run migrations, exit 0/1
//!   tabvault-host --version     # print version

use protocol::{ExtensionMessage, HostMessage, LiveAppSettings, RestoreJob};
use std::io::{Read, Write};
use std::path::PathBuf;
use tv_core::db::{open_vault, write};
use tv_core::error::{Error, Result};
use tv_core::model::SnapshotReason;

mod protocol;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const APP_NAME: &str = "TabVault";
/// 64 MiB is the browser's hard limit for one message; keep ours well under.
const MAX_MESSAGE: u32 = 64 * 1024 * 1024;
/// Jobs for a single poll; the extension drains.
const RESTORE_POLL_LIMIT: u32 = 10;

fn main() {
    // Native-messaging hosts are usually run with no console window; logging is
    // best-effort on both Windows and Unix.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .try_init();

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--version") | Some("-v") => {
            println!("{APP_NAME} native host {APP_VERSION}");
            return;
        }
        Some("--selftest") => {
            std::process::exit(selftest());
        }
        Some(other) => {
            eprintln!("[{APP_NAME}] unknown argument: {other}");
            std::process::exit(2);
        }
        None => {}
    }

    if let Err(e) = run_loop() {
        eprintln!("[{APP_NAME}] fatal: {e}");
        std::process::exit(1);
    }
}

/// The vault path. `TABVAULT_DB` overrides it (tests, installer validation).
fn vault_path() -> Result<PathBuf> {
    if let Some(override_path) = std::env::var_os("TABVAULT_DB") {
        return Ok(PathBuf::from(override_path));
    }
    let appdata = std::env::var_os("APPDATA").ok_or_else(|| {
        Error::Other("APPDATA is not set; the vault has nowhere to live".into())
    })?;
    Ok(PathBuf::from(appdata).join("TabVault").join("tabvault.db"))
}

/// `--selftest`: prove the vault can be opened and migrated, and print one line.
/// Used by the installer before it declares success.
fn selftest() -> i32 {
    match vault_path().and_then(|p| {
        // Smoke test must not create a vault the user never wanted; if it does
        // not exist yet, create it temporarily and report.
        let existed = p.is_file();
        let db = open_vault(&p)?;
        let integrity = db.integrity_check()?;
        let mode = if db.is_encrypted() { "encrypted" } else { "plaintext" };
        println!(
            "ok: opened {} vault at {} ({}), schema migrations applied",
            mode,
            p.display(),
            integrity.join(", ")
        );
        if !existed {
            eprintln!("note: selftest created a fresh vault at {}", p.display());
        }
        Ok::<(), Error>(())
    }) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("selftest FAILED: {e}");
            1
        }
    }
}

/// Frames one message from raw stdin/stdout.
fn read_frame(stdin: &mut std::io::Stdin) -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match stdin
        .read_exact(&mut len_buf)
        .map(Some)
        .or_else(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => Ok(None),
            _ => Err(e),
        })? {
        Some(()) => {}
        None => return Ok(None),
    }
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_MESSAGE {
        return Err(Error::Other(format!(
            "native message claims to be {len} bytes (limit {MAX_MESSAGE}); refusing to read"
        )));
    }
    let mut buf = vec![0u8; len as usize];
    stdin
        .read_exact(&mut buf)
        .map_err(|e| Error::Other(format!("failed reading native frame body: {e}")))?;
    Ok(Some(buf))
}

fn write_frame(stdout: &mut std::io::Stdout, msg: &HostMessage) -> Result<()> {
    let body = serde_json::to_vec(msg)?;
    let len = u32::try_from(body.len())
        .map_err(|_| Error::Other("response frame exceeds the 4 GiB frame length field".into()))?;
    if len > MAX_MESSAGE {
        return Err(Error::Other(format!(
            "response frame is {len} bytes (limit {MAX_MESSAGE})"
        )));
    }
    stdout
        .write_all(&len.to_le_bytes())
        .map_err(|e| Error::Other(format!("could not write frame length: {e}")))?;
    stdout
        .write_all(&body)
        .map_err(|e| Error::Other(format!("could not write frame body: {e}")))?;
    stdout
        .flush()
        .map_err(|e| Error::Other(format!("could not flush stdout: {e}")))?;
    Ok(())
}

/// Opens the vault; on encryption failure we must NOT silently fall back to a
/// plaintext file, so we surface the error to the extension and exit.
fn open_or_report(stdout: &mut std::io::Stdout) -> Result<tv_core::db::Db> {
    let path = vault_path()?;
    open_vault(&path).map_err(|e| {
        let _ = write_frame(
            stdout,
            &HostMessage::error(
                "vault_unavailable",
                format!(
                    "TabVault could not open its database at {}.\n{e}",
                    path.display()
                ),
            ),
        );
        e
    })
}

fn run_loop() -> Result<()> {
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    // Open lazily on the first message so that a bare `--version` invocation
    // (used by the installer) never touches the database.
    let mut db = open_or_report(&mut stdout)?;

    loop {
        let Some(frame) = read_frame(&mut stdin)? else {
            break;
        };
        let reply = match serde_json::from_slice::<ExtensionMessage>(&frame) {
            Ok(msg) => handle_message(msg, &mut db, &mut stdout),
            Err(e) => Ok(Some(HostMessage::error(
                "bad_message",
                format!("unrecognised message from extension: {e}"),
            ))),
        };
        match reply {
            Ok(Some(msg)) => write_frame(&mut stdout, &msg)?,
            Ok(None) => {}
            Err(e) => {
                let _ = write_frame(
                    &mut stdout,
                    &HostMessage::error("host_failure", format!("{e}")),
                );
            }
        }
    }
    Ok(())
}

fn handle_message(
    msg: ExtensionMessage,
    db: &mut tv_core::db::Db,
    stdout: &mut std::io::Stdout,
) -> Result<Option<HostMessage>> {
    match msg {
        ExtensionMessage::Hello {
            browser,
            profile,
            extension_version,
        } => {
            tracing::info!(%browser, %profile, %extension_version, "extension connected");
            let _ = stdout.flush();
            Ok(Some(HostMessage::HelloAck {
                app_version: APP_VERSION.to_string(),
                capabilities: vec![
                    "state".into(),
                    "poll_restore".into(),
                    "notify".into(),
                    "recovery_point".into(),
                ],
            }))
        }

        ExtensionMessage::State { snapshot, reason } => {
            let parsed_reason = parse_reason(&reason);
            let report = write::ingest_full_state(db, &snapshot, parsed_reason)?;
            let (snapshot_id, unchanged, recovery_point) = match &report.outcome {
                tv_core::db::write::IngestOutcome::Paused => {
                    return Ok(Some(HostMessage::Ack {
                        snapshot_id: None,
                        unchanged: true,
                        recovery_point: false,
                    }));
                }
                tv_core::db::write::IngestOutcome::Unchanged { snapshot_id } => {
                    (Some(*snapshot_id), true, false)
                }
                tv_core::db::write::IngestOutcome::Stored {
                    snapshot_id,
                    recovery_point,
                    ..
                } => (Some(*snapshot_id), false, *recovery_point),
            };
            // Notify on a recovery point: many tabs vanished at once.
            if recovery_point {
                let count = report.closed_tabs.len();
                let windows = report.closed_windows;
                let (title, body) = if windows > 0 {
                    (
                        format!("{windows} window(s) just closed"),
                        format!("{count} tabs were in them. Open TabVault to restore?"),
                    )
                } else {
                    (
                        format!("{count} tabs just closed"),
                        "TabVault flagged this as a recovery point. Open TabVault to restore?".into(),
                    )
                };
                let _ = write_frame(stdout, &HostMessage::Notify { title, body, tag: format!("recovery-{snapshot_id:?}") });
            }
            Ok(Some(HostMessage::Ack {
                snapshot_id,
                unchanged,
                recovery_point,
            }))
        }

        ExtensionMessage::PollRestore { limit } => {
            // The extension asks; we drain only when it asks (never otherwise),
            // so the host never spams the browser.
            let jobs = write::drain_restore_requests(db, limit.min(RESTORE_POLL_LIMIT))?;
            let jobs = jobs
                .into_iter()
                .map(|(id, action, payload)| RestoreJob {
                    id,
                    action,
                    payload: serde_json::from_str(&payload)
                        .unwrap_or_else(|_| serde_json::json!({ "raw": payload })),
                })
                .collect();
            // Push the authoritative live settings with every poll reply.
            let settings = live_settings(db);
            Ok(Some(HostMessage::Restore {
                jobs,
                settings: Some(settings),
            }))
        }

        ExtensionMessage::RestoreDone { job_id, error } => {
            write::finish_restore_request(db, job_id, error.as_deref())?;
            Ok(None)
        }

        ExtensionMessage::SetPaused { paused } => {
            write::set_recording_paused(db, paused)?;
            Ok(Some(HostMessage::AckSettings {
                settings: live_settings(db),
            }))
        }

        ExtensionMessage::ImportSessions { entries } => {
            let report = write::import_session_entries(db, "browser", "default", &entries)?;
            tracing::info!(found = report.items_found, imported = report.items_imported, "imported session entries");
            Ok(Some(HostMessage::AckSettings {
                settings: live_settings(db),
            }))
        }

        ExtensionMessage::ImportHistory { entries } => {
            let report = write::import_history_entries(db, "browser", "default", &entries)?;
            tracing::info!(found = report.items_found, imported = report.items_imported, "seeded history stats");
            Ok(Some(HostMessage::AckSettings {
                settings: live_settings(db),
            }))
        }

        ExtensionMessage::OpenApp => {
            open_desktop_app();
            Ok(None)
        }

        ExtensionMessage::Ping => Ok(Some(HostMessage::Pong {
            app_version: APP_VERSION.to_string(),
        })),
    }
}

fn live_settings(db: &tv_core::db::Db) -> LiveAppSettings {
    let s = db.settings();
    LiveAppSettings {
        paused: s.recording_paused,
        capture_interval_secs: s.capture_interval_secs,
        capture_incognito: s.capture_incognito,
        notify_on_mass_close: s.notify_on_mass_close,
        mass_close_tab_threshold: s.mass_close_tab_threshold,
        mass_close_window_secs: s.mass_close_window_secs,
    }
}

/// Launches the desktop app, found next to this binary (the installer places
/// them in the same directory).
fn open_desktop_app() {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("tabvault.exe")));
    match exe {
        Some(path) if path.is_file() => {
            let _ = std::process::Command::new(&path).spawn();
        }
        _ => {
            eprintln!("[tabvault] open_app requested but tabvault.exe was not found next to the host");
        }
    }
}

fn parse_reason(s: &str) -> SnapshotReason {
    SnapshotReason::parse(s).unwrap_or(SnapshotReason::Interval)
}
