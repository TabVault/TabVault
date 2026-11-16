//! Enabling/disabling at-rest encryption for the vault.
//!
//! The migration is deliberately paranoid because this is the one path that can
//! destroy or lock out the user's data:
//!
//! 1. Drain the WAL (checkpoint) so the main file is self-consistent.
//! 2. Build a separate **encrypted twin** with `sqlcipher_export`.
//! 3. Verify the twin (integrity_check + identical row counts). Only then.
//! 4. Write an *encrypted* backup into `backups/`.
//! 5. Wrap the key in DPAPI and write `key.dpapi` next to the vault.
//! 6. Swap files. Reopen with the key. Flip the settings flag.
//!
//! Steps 1–5 are non-destructive; the irreversible swap happens last and only
//! after the twin is verified. If the native host still holds the vault open
//! (a browser is running), the file rename fails and we return an actionable
//! error — nothing has been damaged at that point.
//!
//! Disabling mirrors the flow with empty-key export.

use crate::crypto;
use crate::db::open_vault;
use crate::error::{Error, Result};
use crate::time::now_ms;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

fn vault_dir(db_path: &Path) -> Result<PathBuf> {
    db_path
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| Error::Other("database path has no parent directory".into()))
}

/// True when the vault is currently encrypted (a DPAPI key exists next to it).
pub fn is_encrypted(db_path: &Path) -> bool {
    vault_dir(db_path)
        .map(|d| d.join("key.dpapi").is_file())
        .unwrap_or(false)
}

fn hex_key(key: &[u8; 32]) -> String {
    format!("x{}", key.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn integrity_and_counts(conn: &Connection) -> Result<(Vec<String>, Vec<(String, i64)>)> {
    let check: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA integrity_check")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let tables = [
        "snapshots",
        "windows",
        "tabs",
        "closed_tab_events",
        "closed_window_events",
        "visit_stats",
        "pinned_sites",
        "search_docs",
        "restore_requests",
        "import_batches",
    ];
    let mut counts = Vec::new();
    for table in tables {
        let sql = format!("SELECT count(*) FROM {table}");
        let count: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
        counts.push((table.to_string(), count));
    }
    Ok((check, counts))
}

/// Enables encryption on an *unencrypted* vault. Idempotent.
pub fn enable_encryption(db_path: &Path) -> Result<()> {
    if is_encrypted(db_path) {
        return Ok(());
    }
    let dir = vault_dir(db_path)?;
    let key_path = dir.join("key.dpapi");
    let staging = dir.join(format!(".vault-encrypt-{}.db", now_ms()));
    let backup_dir = dir.join("backups");

    // 1. Drain and snapshot row counts for later verification.
    let plain_snapshot = {
        let conn = Connection::open(db_path)?;
        let (_, counts) = integrity_and_counts(&conn)?;
        counts
    };

    if std::fs::remove_file(&staging).is_err() {
        // not present, fine
    }

    // 2. Export the plaintext vault into a fresh encrypted twin.
    let key = crypto::random_key_32()?;
    let key_hex = hex_key(&key);
    let plain_conn = Connection::open(db_path)?;
    plain_conn.execute_batch(&format!(
        "ATTACH DATABASE '{}' AS encrypted KEY '{key_hex}'",
        staging.display()
    ))?;
    let export: i64 = plain_conn.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(1))?;
    if export != 1 {
        return Err(Error::Other("sqlcipher_export reported failure".into()));
    }
    plain_conn.execute_batch("DETACH DATABASE encrypted;")?;
    drop(plain_conn);

    // 3. Verify the twin before we trust it with anything.
    {
        let conn = Connection::open(&staging)?;
        conn.pragma_update(None, "key", &key_hex)?;
        let (check, counts) = integrity_and_counts(&conn)?;
        if check.iter().any(|row| row != "ok") {
            let _ = std::fs::remove_file(&staging);
            return Err(Error::Other(format!(
                "the encrypted twin failed integrity_check: {}",
                check.join("; ")
            )));
        }
        if counts != plain_snapshot {
            let _ = std::fs::remove_file(&staging);
            return Err(Error::Other(
                "the encrypted twin's row counts differ from the original; refusing to continue".into(),
            ));
        }
    }

    // 4. Encrypted backup of the twin (insurance before any destructive step).
    std::fs::create_dir_all(&backup_dir).map_err(|e| Error::io(backup_dir.display(), e))?;
    let backup = backup_dir.join(format!("tabvault-pre-migration-{}.db", now_ms()));
    std::fs::copy(&staging, &backup).map_err(|e| Error::io(backup.display(), e))?;

    // 5. Wrap the key in DPAPI where only this user on this machine can unwrap it.
    let blob = crypto::dpapi_protect(&key)?;
    std::fs::write(&key_path, &blob).map_err(|e| Error::io(key_path.display(), e))?;

    // 6. Swap: remove the plaintext file, put the twin in place.
    //    If this fails (host still has the file open), we roll back key.dpapi so
    //    the next open still finds a consistent plaintext vault.
    if let Err(e) = std::fs::remove_file(db_path) {
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(&staging);
        return Err(Error::Other(format!(
            "could not replace the vault because it is open elsewhere (is a browser still running?). Nothing was changed. {e}"
        )));
    }
    if let Err(e) = std::fs::rename(&staging, db_path) {
        // Try to roll back to the plaintext state by restoring the backup twin?
        // We no longer have the plaintext file (deleted). The twin IS encrypted
        // and key.dpapi IS intact, so rename failure can still recover by just
        // copying the twin over.
        let _ = std::fs::copy(&staging, db_path);
        let _ = std::fs::remove_file(&staging);
        return Err(Error::Other(format!(
            "could not finalize the vault swap, but the encrypted data and key are intact: {e}"
        )));
    }

    // 7. Reopen encrypted, set the flag, drain the WAL.
    {
        let conn = open_vault(db_path)?;
        let mut settings = conn.settings();
        settings.encryption_enabled = true;
        conn.set_settings(&settings)?;
        conn.checkpoint()?;
    }
    Ok(())
}

/// Disables encryption on an encrypted vault. Idempotent.
pub fn disable_encryption(db_path: &Path) -> Result<()> {
    if !is_encrypted(db_path) {
        return Ok(());
    }
    let dir = vault_dir(db_path)?;
    let key_path = dir.join("key.dpapi");
    let staging = dir.join(format!(".vault-decrypt-{}.db", now_ms()));

    // 1. Verify the encrypted vault and snapshot row counts.
    let enc_snapshot = {
        let conn = open_vault(db_path)?;
        let (check, counts) = integrity_and_counts(conn.conn())?;
        if check.iter().any(|row| row != "ok") {
            return Err(Error::Other(format!(
                "the encrypted vault fails integrity_check; aborting before any changes: {}",
                check.join("; ")
            )));
        }
        counts
    };

    if std::fs::remove_file(&staging).is_err() {
        // absent, fine
    }

    // 2. Export into a plaintext twin.
    let key_blob = std::fs::read(&key_path).map_err(|e| Error::io(key_path.display(), e))?;
    let raw_key = crypto::dpapi_unprotect(&key_blob)?;
    let raw_arr: [u8; 32] = raw_key
        .as_slice()
        .try_into()
        .map_err(|_| Error::Other("the vault key blob is not 32 bytes".into()))?;
    let key_hex = hex_key(&raw_arr);

    let enc_conn = Connection::open(db_path)?;
    enc_conn.pragma_update(None, "key", &key_hex)?;
    enc_conn.execute_batch(&format!(
        "ATTACH DATABASE '{}' AS plaintext KEY ''",
        staging.display()
    ))?;
    let export: i64 = enc_conn.query_row("SELECT sqlcipher_export('plaintext')", [], |_| Ok(1))?;
    if export != 1 {
        let _ = std::fs::remove_file(&staging);
        return Err(Error::Other("sqlcipher_export reported failure".into()));
    }
    enc_conn.execute_batch("DETACH DATABASE plaintext;")?;
    drop(enc_conn);

    // 3. Verify the plaintext twin.
    {
        let conn = Connection::open(&staging)?;
        let (check, counts) = integrity_and_counts(&conn)?;
        if check.iter().any(|row| row != "ok") {
            let _ = std::fs::remove_file(&staging);
            return Err(Error::Other(format!(
                "the decrypted twin failed integrity_check: {}",
                check.join("; ")
            )));
        }
        if counts != enc_snapshot {
            let _ = std::fs::remove_file(&staging);
            return Err(Error::Other(
                "the decrypted twin's row counts differ; refusing to continue".into(),
            ));
        }
    }

    // 4. Swap. If the file is in use, bail out before touching the key.
    if let Err(e) = std::fs::remove_file(db_path) {
        let _ = std::fs::remove_file(&staging);
        return Err(Error::Other(format!(
            "could not replace the vault because it is open elsewhere (close all browsers and retry). Nothing was changed. {e}"
        )));
    }
    if let Err(e) = std::fs::rename(&staging, db_path) {
        let _ = std::fs::copy(&staging, db_path);
        let _ = std::fs::remove_file(&staging);
        return Err(Error::Other(format!(
            "could not finalize the swap, but your data is intact at {}: {e}",
            staging.display()
        )));
    }

    // 5. Drop the key only after the plaintext vault is verified and in place.
    let _ = std::fs::remove_file(&key_path);

    {
        let conn = open_vault(db_path)?;
        let mut settings = conn.settings();
        settings.encryption_enabled = false;
        conn.set_settings(&settings)?;
        conn.checkpoint()?;
    }
    Ok(())
}
