//! Cryptographic helpers.
//!
//! Two distinct jobs, deliberately kept apart:
//!
//! 1. **DPAPI** (Windows only): protects the small secret that unlocks the
//!    encrypted vault, and unwraps Chrome/Edge OSCrypt keys (`v10`) for the
//!    session-file recovery tool.
//! 2. **AES-256-GCM**: used by OSCrypt. Implemented with the audited pure-Rust
//!    `aes-gcm` crate rather than hand-rolled windows
//!    `BCryptEncrypt`/`BCryptDecrypt` plumbing.
//!
//! Nothing here ever leaves the machine; there is no network code in this crate.

use crate::error::{Error, Result};

const DPAPI_PREFIX: &[u8] = b"DPAPI";
const V10_PREFIX: &[u8] = b"v10";
const V20_PREFIX: &[u8] = b"v20";
const ABE_PREFIX: &[u8] = b"APPBOUND";

/// Unprotects a DPAPI blob in the *current user's* scope.
///
/// `data` is the raw bytes passed to `CryptUnprotectData` (e.g. the output of
/// `CryptProtectData`, or a Local State `encrypted_key` value after its
/// `DPAPI` prefix has been stripped).
#[cfg(windows)]
pub fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    let mut in_blob = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    // SAFETY: both blobs are plain byte pointer/length pairs; we pass
    // CRYPTPROTECT_UI_FORBIDDEN so the call can never show UI (it must not block
    // a background tool). On failure we leak nothing and report the status.
    let ok = unsafe {
        CryptUnprotectData(
            &mut in_blob,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if ok == 0 {
        return Err(Error::Other(format!(
            "CryptUnprotectData failed (Windows error {}). The secret was encrypted for a different Windows user or machine.",
            unsafe { windows_sys::Win32::Foundation::GetLastError() }
        )));
    }
    let bytes = if out_blob.pbData.is_null() {
        Vec::new()
    } else {
        // SAFETY: pbData points to cbData bytes allocated by the OS, and the
        // length comes straight from the OS.
        unsafe { std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec() }
    };
    if !out_blob.pbData.is_null() {
        // SAFETY: out_blob.pbData was returned by CryptUnprotectData and must be
        // released with LocalFree.
        unsafe { LocalFree(out_blob.pbData as HLOCAL) };
    }
    Ok(bytes)
}

/// DPAPI is a Windows API; on other platforms this always fails with a clear
/// error instead of silently pretending to succeed.
#[cfg(not(windows))]
pub fn dpapi_unprotect(_data: &[u8]) -> Result<Vec<u8>> {
    Err(Error::Unsupported(
        "DPAPI is only available on Windows".to_string(),
    ))
}

/// Protects bytes with DPAPI in the current user's scope.
#[cfg(windows)]
pub fn dpapi_protect(data: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    let mut in_blob = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptProtectData(
            &mut in_blob,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
    };
    if ok == 0 {
        return Err(Error::Other(format!(
            "CryptProtectData failed (Windows error {})",
            unsafe { windows_sys::Win32::Foundation::GetLastError() }
        )));
    }
    let bytes = unsafe {
        if out_blob.pbData.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec()
        }
    };
    if !out_blob.pbData.is_null() {
        unsafe { LocalFree(out_blob.pbData as HLOCAL) };
    }
    Ok(bytes)
}

#[cfg(not(windows))]
pub fn dpapi_protect(_data: &[u8]) -> Result<Vec<u8>> {
    Err(Error::Unsupported(
        "DPAPI is only available on Windows".to_string(),
    ))
}

/// Returns a fresh 32-byte AES key, suitable as an OSCrypt substitute or as the
/// vault key before DPAPI wrapping.
pub fn random_key_32() -> Result<[u8; 32]> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::Cryptography::{
            BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        };
        let mut buf = [0u8; 32];
        // SAFETY: passing a valid mutable byte buffer with its length; the
        // status code is checked.
        let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        if status < 0 {
            return Err(Error::Other(format!("BCryptGenRandom failed ({status})")));
        }
        return Ok(buf);
    }
    #[cfg(not(windows))]
    {
        // Outside Windows use blake3 over OS randomness from /dev/urandom.
        use std::io::Read;
        let mut buf = [0u8; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut buf))
            .map_err(|e| Error::io("/dev/urandom", e))?;
        Ok(buf)
    }
}

/// Decodes the `encrypted_key` entry from a browser's `Local State` JSON into
/// the raw AES key.
///
/// Local State stores `"encrypted_key": "<base64>"` where the base64 decodes to
/// an ASCII `DPAPI` prefix followed by the DPAPI-protected key bytes. We strip
/// the prefix and unwrap with DPAPI in the user's context.
pub fn unwrap_os_crypt_key(local_state_json: &str) -> Result<[u8; 32]> {
    let json: serde_json::Value = serde_json::from_str(local_state_json)
        .map_err(|e| Error::BadFormat(format!("Local State is not valid JSON: {e}")))?;
    let key_b64 = json
        .get("os_crypt")
        .and_then(|v| v.get("encrypted_key"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::BadFormat("Local State has no os_crypt.encrypted_key".into()))?;
    let raw = base64_decode(key_b64)?;
    if raw.len() <= DPAPI_PREFIX.len() || &raw[..DPAPI_PREFIX.len()] != DPAPI_PREFIX {
        return Err(Error::BadFormat(
            "encrypted_key does not carry the expected DPAPI prefix".into(),
        ));
    }
    let key = dpapi_unprotect(&raw[DPAPI_PREFIX.len()..])?;
    let arr: [u8; 32] = key
        .as_slice()
        .try_into()
        .map_err(|_| Error::BadFormat(format!("unwrapped key was {} bytes, expected 32", key.len())))?;
    Ok(arr)
}

/// Returns `true` when this browser has an App-Bound Encryption key.
///
/// App-Bound (`APPBOUND`) keys are released only to the browser itself (or to
/// SYSTEM for the initial unwrap). A third-party tool in the same user session
/// cannot unwrap them, so attempting to decrypt `v20` blobs would be dishonest —
/// we detect and report instead of producing corrupt data.
pub fn has_app_bound_key(local_state_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(local_state_json)
        .ok()
        .and_then(|j| {
            j.get("os_crypt")
                .and_then(|v| v.get("app_bound_encrypted_key"))
                .and_then(|v| v.as_str())
                .map(|k| base64_decode(k).map(|b| b.starts_with(ABE_PREFIX)).unwrap_or(false))
        })
        .unwrap_or(false)
}

/// Encrypts a payload with AES-256-GCM, returning `v10 || nonce || ct+tag`.
///
/// Present for the inverse round-trip in tests and for exporting diagnostics.
#[cfg(test)]
pub fn aes256_gcm_encrypt(key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {
    use aes_gcm::aead::{Aead, KeyInit};
    let cipher = aes_gcm::Aes256Gcm::new(aes_gcm::Key::<aes_gcm::Aes256Gcm>::from_slice(key));
    let ct = cipher
        .encrypt(aes_gcm::Nonce::from_slice(nonce), plaintext)
        .expect("encryption cannot fail for in-memory plaintext");
    let mut out = Vec::with_capacity(V10_PREFIX.len() + nonce.len() + ct.len());
    out.extend_from_slice(V10_PREFIX);
    out.extend_from_slice(nonce);
    out.extend_from_slice(&ct);
    out
}

/// Decrypts an OSCrypt `v10` blob with the given key.
///
/// Layout: `"v10" || nonce(12) || ciphertext || tag(16)`.
pub fn decrypt_os_crypt_v10(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit};

    const HEADER: usize = 3 + 12; // "v10" + nonce
    if blob.len() < HEADER + 16 {
        return Err(Error::BadFormat(format!("v10 blob too short ({} bytes)", blob.len())));
    }
    if !blob.starts_with(V10_PREFIX) {
        return Err(Error::BadFormat("not a v10 blob".into()));
    }
    let nonce = &blob[3..15];
    let ct = &blob[15..];
    let cipher = aes_gcm::Aes256Gcm::new(aes_gcm::Key::<aes_gcm::Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(aes_gcm::Nonce::from_slice(nonce), ct)
        .map_err(|_| Error::Other("AES-256-GCM authentication failed; the key does not match this blob".into()))
}

/// True when the blob starts with the app-bound `v20` prefix.
pub fn is_app_bound_blob(blob: &[u8]) -> bool {
    blob.starts_with(V20_PREFIX)
}

/// Decodes standard base64, accepting the padding variants browsers emit.
pub fn base64_decode(input: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(&cleaned)
        .map_err(|e| Error::BadFormat(format!("invalid base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        let raw = b"DPAPI-secret-bytes\x00\x01";
        use base64::Engine;
        let enc = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(base64_decode(&enc).unwrap(), raw);
    }

    #[test]
    fn base64_rejects_garbage() {
        assert!(base64_decode("!!!not-base64!!!").is_err());
    }

    #[test]
    fn v10_roundtrip() {
        let key = [7u8; 32];
        let nonce = [9u8; 12];
        let pt = b"id\x00\x00\x00payload-bytes";
        let blob = aes256_gcm_encrypt(&key, &nonce, pt);
        assert!(blob.starts_with(b"v10"));
        assert_eq!(decrypt_os_crypt_v10(&key, &blob).unwrap(), pt);
        // Wrong key must fail authentication, never return "decrypted" garbage.
        let bad_key = [8u8; 32];
        assert!(decrypt_os_crypt_v10(&bad_key, &blob).is_err());
    }

    #[test]
    fn v10_truncated_blob_is_rejected() {
        let key = [7u8; 32];
        assert!(decrypt_os_crypt_v10(&key, b"v10").is_err());
        assert!(decrypt_os_crypt_v10(&key, b"v11xxxxxxxxxx").is_err());
    }
}
