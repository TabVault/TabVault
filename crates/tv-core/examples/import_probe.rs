//! Developer probe: run the full recover-from-the-past import pipeline against
//! a real file (the same code path the desktop app uses) and print a summary.
//!
//! Usage:
//!   cargo run -p tv-core --example import_probe -- <file> <browser> <profile> [--keep]
//!
//! The import goes into a scratch vault in the current working directory so it
//! never touches the user's real vault.

use std::path::PathBuf;
use tv_core::db::{open_vault, write};
use tv_core::model::TabRecord;
use tv_core::parsers::snss::{parse, SessionFileKind};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: import_probe <file> <browser> <profile> [--kind tabs|session]");
    let browser = args.next().unwrap_or_else(|| "chrome".to_string());
    let profile = args.next().unwrap_or_else(|| "Default".to_string());
    let mut kind = SessionFileKind::from_filename(
        std::path::Path::new(&path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(""),
    );
    while let Some(flag) = args.next() {
        if flag == "--kind" {
            kind = match args.next().as_deref() {
                Some("tabs") => Some(SessionFileKind::TabRestore),
                Some("session") => Some(SessionFileKind::SessionService),
                other => panic!("unknown --kind {other:?}"),
            };
        }
    }
    let _keep = false;

    let bytes = std::fs::read(&path).expect("read file");
    let kind = kind.expect("unknown file kind (pass --kind tabs|session)");

    // 1. Parse.
    let recovery = parse(&bytes, kind).expect("parse");
    println!("== parse ==");
    println!("version: {}", recovery.version);
    println!("windows: {}", recovery.windows.len());
    println!("commands: {}", recovery.commands_total);
    println!("skipped: {:?}", recovery.skipped_commands);
    println!("warnings: {:?}", recovery.warnings);
    let total_tabs: usize = recovery.windows.iter().map(|w| w.tabs.len()).sum::<usize>()
        + recovery.closed_tabs.len();
    println!("total tabs recoverable: {total_tabs}");

    // 2. Import into a scratch vault, the same way the app does.
    let scratch = PathBuf::from("target/import-probe");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let vault_path = scratch.join("probe.db");
    let db = open_vault(&vault_path).expect("open scratch vault");

    let batch = write::begin_import_batch(&db, "snss", Some(std::path::Path::new(&path)), Some(&browser), Some(&profile))
        .expect("begin batch");
    let mut imported = 0usize;
    for w in &recovery.windows {
        let tabs: Vec<TabRecord> = w
            .tabs
            .iter()
            .map(|t| t.to_tab_record(&browser, &profile, true))
            .collect();
        if tabs.len() > 1 || w.closing {
            write::import_recovered_window(
                &db,
                batch,
                &browser,
                &profile,
                &w.window_id,
                w.closed_ms,
                &tabs,
                "chromium_snss",
            )
            .expect("import window");
            imported += tabs.len();
        } else if let Some(tab) = tabs.into_iter().next() {
            let stats = write::import_recovered_tabs(
                &db,
                batch,
                "chromium_snss",
                &browser,
                &profile,
                &[(tab, w.closed_ms)],
                false,
            )
            .expect("import tab");
            imported += stats.items_imported;
        }
    }
    for t in &recovery.closed_tabs {
        let tab = t.to_tab_record(&browser, &profile, false);
        let stats = write::import_recovered_tabs(
            &db,
            batch,
            "chromium_snss",
            &browser,
            &profile,
            &[(tab, t.closed_ms)],
            false,
        )
        .expect("import tab");
        imported += stats.items_imported;
    }
    write::finish_import_batch(&db, batch, None).expect("finish batch");

    let closed: i64 = db
        .conn()
        .query_row("SELECT count(*) FROM closed_tab_events", [], |r| r.get(0))
        .unwrap();
    let windows: i64 = db
        .conn()
        .query_row("SELECT count(*) FROM closed_window_events", [], |r| r.get(0))
        .unwrap();
    let pinned: i64 = db
        .conn()
        .query_row("SELECT count(*) FROM pinned_sites", [], |r| r.get(0))
        .unwrap();
    let search: i64 = db
        .conn()
        .query_row("SELECT count(*) FROM search_docs", [], |r| r.get(0))
        .unwrap();

    println!("\n== import ==");
    println!("closed_tab_events: {closed}");
    println!("closed_window_events: {windows}");
    println!("pinned_sites: {pinned}");
    println!("search_docs: {search}");
    println!("scratch vault at {} — delete it any time", vault_path.display());
}
