//! Copying database files that are open in the browser.
//!
//! Browsers hold their `History`/`places.sqlite` open for the whole session,
//! and their WAL may be mid-checkpoint at any time. Reading in place can serve
//! a torn mix of main-file and WAL pages. The correct approach — and the one
//! the spec calls for — is to copy the file **and its `-wal`/`-shm` siblings**
//! to scratch, then open the copy. SQLite then replays the copied WAL normally
//! and sees a consistent database.
//!
//! The browser's own files are never modified.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};

/// Result of a locked-copy, including which siblings existed.
#[derive(Debug, Clone)]
pub struct LockedCopy {
    /// Path to the copied main database file.
    pub path: PathBuf,
    /// Files copied alongside (`-wal`, `-shm`).
    pub sidecars: Vec<PathBuf>,
}

/// Copies `src` and any `-wal`/`-shm` siblings into `scratch_dir`.
///
/// `scratch_dir` is created if needed; callers usually point it at a fresh
/// temp folder per import so stale copies never linger.
pub fn copy_locked_db(src: &Path, scratch_dir: &Path) -> Result<LockedCopy> {
    if !src.is_file() {
        return Err(Error::NotFound(format!(
            "database file not found: {}",
            src.display()
        )));
    }
    std::fs::create_dir_all(scratch_dir).map_err(|e| Error::io(scratch_dir.display(), e))?;

    // Preserve the sidecar names SQLite expects: `<name>-wal`, `<name>-shm`.
    let base_name = src
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "db".to_string());
    // Use a unique target name so concurrent imports never clash.
    let nonce = crate::time::now_ms();
    let target = scratch_dir.join(format!("{base_name}.{nonce}.copy"));

    std::fs::copy(src, &target).map_err(|e| {
        Error::Other(format!(
            "could not copy {} -> {}: {e}. The browser may be holding it open with no read access; close the browser and retry.",
            src.display(),
            target.display()
        ))
    })?;

    let mut sidecars = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let sidecar_src = PathBuf::from(format!("{}{}", src.display(), suffix));
        if sidecar_src.is_file() {
            let sidecar_dst = PathBuf::from(format!("{}{}", target.display(), suffix));
            match std::fs::copy(&sidecar_src, &sidecar_dst) {
                Ok(_) => sidecars.push(sidecar_dst),
                Err(e) => {
                    // A sidecar we cannot copy must not silently produce a
                    // half-consistent database; report it clearly.
                    return Err(Error::Other(format!(
                        "could not copy sidecar {}: {e}",
                        sidecar_src.display()
                    )));
                }
            }
        }
    }
    Ok(LockedCopy {
        path: target,
        sidecars,
    })
}

/// Copies a locked DB and opens the copy read-only, running `f` on the
/// connection. The scratch copy is deleted afterwards.
pub fn with_locked_db_copy<T>(
    src: &Path,
    scratch_dir: &Path,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T>,
) -> Result<T> {
    let copy = copy_locked_db(src, scratch_dir)?;
    let outcome = (|| {
        let conn = rusqlite::Connection::open_with_flags(
            &copy.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        // A copied WAL may reference frames beyond the main file's length;
        // SQLite validates it on first access, and returning that error is the
        // caller's cue to take the fallback path (open read-write or retry).
        f(&conn)
    })();
    // Best-effort cleanup. If the scratch dir is shared, leave a note rather
    // than deleting something another thread may be reading.
    let _ = std::fs::remove_file(&copy.path);
    for sc in &copy.sidecars {
        let _ = std::fs::remove_file(sc);
    }
    outcome
}
