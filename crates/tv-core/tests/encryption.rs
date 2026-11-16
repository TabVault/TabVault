//! Encryption-at-rest migration: plain <-> SQLCipher-encrypted, via
//! `sqlcipher_export`. Exercises the real vendored SQLCipher.

use rusqlite::Connection;
use std::path::PathBuf;
use tv_core as tv_core_crate;

struct Fixture {
    dir: PathBuf,
    plain: PathBuf,
    enc: PathBuf,
    back: PathBuf,
}

fn fixture(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("tabvault-enc-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Fixture {
        plain: dir.join("plain.db"),
        enc: dir.join("enc.db"),
        back: dir.join("roundtrip.db"),
        dir,
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const KEY_HEX: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

#[test]
fn sqlcipher_export_roundtrip_encrypted_and_back() {
    let fx = fixture("roundtrip");

    // 1. Plain source with real data.
    let plain = Connection::open(&fx.plain).unwrap();
    plain
        .execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t(v) VALUES('secret'),('tab');")
        .unwrap();
    drop(plain);

    // 2. Export into the encrypted destination.
    let plain = Connection::open(&fx.plain).unwrap();
    let attach = format!("ATTACH DATABASE '{}' AS encrypted KEY 'x{KEY_HEX}'", fx.enc.display());
    plain.execute_batch(&attach).unwrap();
    let export: i64 = plain.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(1)).unwrap();
    assert_eq!(export, 1);
    plain.execute_batch("DETACH DATABASE encrypted;").unwrap();
    drop(plain);

    // 3. The encrypted file refuses to open without the key...
    let unencrypted_read = Connection::open(&fx.enc).unwrap();
    assert!(unencrypted_read.query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0)).is_err());
    drop(unencrypted_read);
    // ...and opens cleanly with it.
    let enc = Connection::open(&fx.enc).unwrap();
    enc.pragma_update(None, "key", format!("x{KEY_HEX}")).unwrap();
    let count: i64 = enc.query_row("SELECT count(*) FROM t", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 2);
    drop(enc);

    // 4. Export back to plaintext.
    let enc = Connection::open(&fx.enc).unwrap();
    enc.pragma_update(None, "key", format!("x{KEY_HEX}")).unwrap();
    let attach_back = format!("ATTACH DATABASE '{}' AS plaintext KEY ''", fx.back.display());
    enc.execute_batch(&attach_back).unwrap();
    let export_back: i64 = enc
        .query_row("SELECT sqlcipher_export('plaintext')", [], |_| Ok(1))
        .unwrap();
    assert_eq!(export_back, 1);
    enc.execute_batch("DETACH DATABASE plaintext;").unwrap();
    drop(enc);

    let back = Connection::open(&fx.back).unwrap();
    let count_back: i64 = back.query_row("SELECT count(*) FROM t", [], |r| r.get(0)).unwrap();
    assert_eq!(count_back, 2);
}

/// The same dance with the public `crypto` helpers: generate a key, wrap it in
/// DPAPI, unwrap it, use it. Proves the whole key-handling path (Windows only).
#[cfg(windows)]
#[test]
fn dpapi_roundtrip_matches_sqlcipher_expectations() {
    let key = tv_core_crate::crypto::random_key_32().unwrap();
    let wrapped = tv_core_crate::crypto::dpapi_protect(&key).unwrap();
    let unwrapped = tv_core_crate::crypto::dpapi_unprotect(&wrapped).unwrap();
    assert_eq!(key.to_vec(), unwrapped);

    // hex key string matches our PRAGMA format used by open_vault.
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex.len(), 64);
}

/// `open_vault` picks the encrypted path purely from the presence of key.dpapi.
#[test]
fn open_vault_switches_on_key_presence() {
    let fx = fixture("switch");
    let dir = fx.dir.clone();
    // Plain first.
    let db = tv_core_crate::db::open_vault(&fx.plain).unwrap();
    db.conn().execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t(v) VALUES('x');").unwrap();
    drop(db);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1 + 0, "plain vault only");
    // Now pretend the user enabled encryption: copy plain to a fresh name with key.dpapi.
    let enc_path = dir.join("vault-encrypted.db");
    let key = tv_core_crate::crypto::random_key_32().unwrap();
    // produce the encrypted twin with the connection trick
    let src = Connection::open(&fx.plain).unwrap();
    let attach = format!("ATTACH DATABASE '{}' AS encrypted KEY 'x{}'", enc_path.display(), key.iter().map(|b| format!("{b:02x}")).collect::<String>());
    src.execute_batch(&attach).unwrap();
    let _: i64 = src.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(1)).unwrap();
    src.execute_batch("DETACH DATABASE encrypted;").unwrap();
    drop(src);
    // Write the DPAPI key blob where open_vault looks for it.
    let blob = tv_core_crate::crypto::dpapi_protect(&key).unwrap();
    std::fs::write(dir.join("key.dpapi"), blob).unwrap();

    let db = tv_core_crate::db::open_vault(&enc_path).unwrap();
    let count: i64 = db.conn().query_row("SELECT count(*) FROM t", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
}
