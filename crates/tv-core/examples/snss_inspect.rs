//! Developer inspector for Chromium SNSS files.
//!
//! Usage:
//!   cargo run -p tv-core --example snss_inspect -- <file> [kind]
//!
//! `kind` is `session` (default) or `tabs`; when omitted it is derived from
//! the filename. This is a verification tool used during development — the
//! parser itself is unit-tested separately — so its output is human-oriented.

use std::collections::BTreeMap;
use tv_core::parsers::snss::{parse, SessionFileKind};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: snss_inspect <file> [session|tabs] [--histogram]");
        std::process::exit(2);
    };

    let mut histogram = false;
    let mut dump = false;
    let mut kind_name: Option<String> = None;
    for arg in args {
        match arg.as_str() {
            "--histogram" => histogram = true,
            "--dump" => dump = true,
            "session" | "tabs" => kind_name = Some(arg),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let kind = match kind_name.as_deref() {
        Some("session") => Some(SessionFileKind::SessionService),
        Some("tabs") => Some(SessionFileKind::TabRestore),
        _ => SessionFileKind::from_filename(
            std::path::Path::new(&path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(""),
        ),
    };
    let Some(kind) = kind else {
        eprintln!("could not infer file kind from {path}; pass 'session' or 'tabs'");
        std::process::exit(2);
    };

    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    eprintln!("file: {}", path);
    eprintln!("kind: {:?}", kind);
    eprintln!("size: {} bytes", bytes.len());
    eprintln!("header: {:?}", &bytes[..8.min(bytes.len())]);

    if dump {
        println!("== raw record sequence ==");
        let mut pos = 8usize;
        let mut i = 0usize;
        while pos + 2 <= bytes.len() {
            let size = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
            if size == 0 || pos + 2 + size > bytes.len() {
                println!("-- truncated at offset {pos} --");
                break;
            }
            let id = bytes[pos + 2];
            let plen = size - 1;
            let p = &bytes[pos + 3..pos + 2 + size];
            let summary = match id {
                0 => format!("window={} tab={}", u32::from_le_bytes([p[0], p[1], p[2], p[3]]), u32::from_le_bytes([p[4], p[5], p[6], p[7]])),
                2 => format!("tab={} index={}", u32::from_le_bytes([p[0], p[1], p[2], p[3]]), i32::from_le_bytes([p[4], p[5], p[6], p[7]])),
                4 => format!("tab={} sel={} ts?", u32::from_le_bytes([p[0], p[1], p[2], p[3]]), i32::from_le_bytes([p[4], p[5], p[6], p[7]])),
                6 => {
                    // Decode tab_id + index, then peek at the url.
                    let tab = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                    let idx = i32::from_le_bytes([p[4], p[5], p[6], p[7]]);
                    // pickle header(4) + tab_id(4) + index(4) = 12; then len(4) + url.
                    if p.len() >= 16 {
                        let off = 16usize;
                        let ulen = u32::from_le_bytes([p[12], p[13], p[14], p[15]]) as usize;
                        let end = (off + ulen.min(96)).min(p.len());
                        let url: String = p[off..end]
                            .iter()
                            .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
                            .collect();
                        format!("tab={tab} index={idx} url={url}")
                    } else {
                        format!("tab={tab} index={idx} (short payload)")
                    }
                }
                12 => format!("tab={} pinned={}", u32::from_le_bytes([p[0], p[1], p[2], p[3]]), p[4]),
                16 => format!("tab={} close_ts?", u32::from_le_bytes([p[0], p[1], p[2], p[3]])),
                17 => format!("window={} close_ts?", u32::from_le_bytes([p[0], p[1], p[2], p[3]])),
                25 => format!("tab={} group?", u32::from_le_bytes([p[0], p[1], p[2], p[3]])),
                _ => String::new(),
            };
            println!("{i:>4} off={pos:>7} id={id:<4} len={plen:>6} {summary}");
            pos += 2 + size;
            i += 1;
        }
        return;
    }

    let recovery = match parse(&bytes, kind) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("\nFAILED: {e}");
            std::process::exit(1);
        }
    };

    if histogram {
        println!("== command histogram (id -> count, payload sizes) ==");
        let mut sizes: BTreeMap<u8, BTreeMap<usize, usize>> = BTreeMap::new();
        // Re-derive by scanning raw records exactly as the parser sees them.
        let mut pos = 8usize;
        while pos + 2 <= bytes.len() {
            let size = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
            if size == 0 || pos + 2 + size > bytes.len() {
                break;
            }
            let id = bytes[pos + 2];
            *sizes.entry(id).or_default().entry(size - 1).or_default() += 1;
            pos += 2 + size;
        }
        for (id, map) in &sizes {
            println!("  id {id:>3}: total {:>5}", map.values().sum::<usize>());
            for (plen, n) in map {
                println!("        payload {plen:>5} x {n}");
            }
        }
        return;
    }

    eprintln!("\n== header version: {} ==", recovery.version);
    eprintln!("== commands total: {} ==", recovery.commands_total);
    eprintln!("== windows: {} ==", recovery.windows.len());
    eprintln!("== closed (loose) tabs: {} ==", recovery.closed_tabs.len());
    eprintln!("== groups: {} ==", recovery.groups.len());
    if !recovery.warnings.is_empty() {
        eprintln!("== warnings ==");
        for w in &recovery.warnings {
            eprintln!("  ! {w}");
        }
    }
    if !recovery.skipped_commands.is_empty() {
        eprintln!("== skipped commands (id -> count) ==");
        for (id, n) in &recovery.skipped_commands {
            eprintln!("  {id:>3} -> {n}");
        }
    }

    for (i, w) in recovery.windows.iter().enumerate() {
        let closed = w.closed_ms.map(|ms| tv_core::time::format_local(ms)).unwrap_or_else(|| "live".into());
        println!("\nWINDOW #{i} id={} closed={closed} tabs={}", w.window_id, w.tabs.len());
        for (j, t) in w.tabs.iter().enumerate() {
            let pin = if t.pinned { "[pinned]" } else { "        " };
            let group = t.group_name.as_deref().unwrap_or("");
            let title = if t.title.len() > 60 { &t.title[..60] } else { &t.title };
            println!("  {j:>2} {pin} {group:>12} | {title} | {}", t.url);
        }
    }

    println!("\nCLOSED LOOSE TABS ({})", recovery.closed_tabs.len());
    for (j, t) in recovery.closed_tabs.iter().enumerate() {
        let pin = if t.pinned { "[pinned]" } else { "        " };
        let when = t.closed_ms.map(tv_core::time::format_local).unwrap_or_else(|| "unknown".into());
        let title = if t.title.len() > 60 { &t.title[..60] } else { &t.title };
        println!("  {j:>2} {pin} {when} | {title} | {}", t.url);
    }

    for g in &recovery.groups {
        println!("\nGROUP {:?} color={} collapsed={} title={}", g.token, g.color, g.collapsed, g.title);
    }
}
