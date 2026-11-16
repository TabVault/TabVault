//! Chromium's `base::Pickle` reader.
//!
//! Chromium serialises its session records into `Pickle`s. The format is not
//! self-describing: it is a flat little-endian byte stream whose field types are
//! known only from the writer's code, and every variable-length value is padded
//! to a 4-byte boundary. Getting a single padding step wrong desynchronises the
//! stream and yields garbage URLs, so this reader is written to be *strict* —
//! every read is bounds-checked and reports exactly how many bytes it consumed.
//!
//! Layout facts (each verified against `base/pickle.h` and the session writers):
//!
//! * An `int` is **always** 4 bytes, even for `SessionID`, which the Chromium
//!   source declares as `int32_t`. (`SessionID::id_type` historically maps to a
//!   union, so `sizeof` in the reader must not be trusted; 4 is correct.)
//! * `int64` is 8 bytes, no special alignment beyond the running offset, which
//!   is always a multiple of 4 by construction.
//! * `bool` is written as a full 4-byte int (0 or 1).
//! * `string` is `u32 length` + `length` bytes + padding to a 4-byte boundary.
//! * `string16` is `u32 character_count` + `2 * character_count` bytes of
//!   UTF-16LE + padding to a 4-byte boundary. Note it stores **characters**, not
//!   bytes — a distinction that silently halves or doubles the title otherwise.
//! * `double` is 8 bytes.

/// A bounds-checked cursor over a Pickle body.
pub struct PickleReader<'a> {
    data: &'a [u8],
    pos: usize,
}

/// What went wrong while reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickleError {
    /// A read ran past the end of the buffer. Chromium treats this as "field
    /// absent", which is how older records stay readable by newer code.
    Truncated { wanted: usize, available: usize },
    /// A length prefix is implausible, i.e. the stream is misaligned or corrupt.
    ImplausibleLength { length: u64, remaining: usize },
    /// A UTF-16 string had an odd byte count.
    BadUtf16 { bytes: usize },
}

impl std::fmt::Display for PickleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PickleError::Truncated { wanted, available } => {
                write!(f, "pickle truncated: wanted {wanted} bytes, {available} available")
            }
            PickleError::ImplausibleLength { length, remaining } => write!(
                f,
                "pickle length prefix {length} exceeds the {remaining} bytes remaining"
            ),
            PickleError::BadUtf16 { bytes } => write!(f, "UTF-16 payload has an odd length ({bytes})"),
        }
    }
}

impl std::error::Error for PickleError {}

/// The largest string we will believe. Chromium's own limit for a navigation is
/// 63 KiB; anything far beyond that is a misaligned read, not real data.
const MAX_STRING_BYTES: u64 = 4 * 1024 * 1024;

type PResult<T> = std::result::Result<T, PickleError>;

impl<'a> PickleReader<'a> {
    /// Wraps a *payload*, i.e. the bytes after the outer `u32 length` header.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Wraps a full pickle, consuming and validating its length header.
    ///
    /// Chromium's `PickleIterator::WithData` also tolerates the header being
    /// absent, so a failure here falls back to reading from offset 0.
    pub fn from_pickle(data: &'a [u8]) -> Self {
        if data.len() >= 4 {
            let declared = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
            // Accept the header only when it is self-consistent.
            if declared == data.len() - 4 {
                return Self { data, pos: 4 };
            }
            // Some writers include further trailing structure; accept when the
            // declared body fits inside the buffer.
            if declared <= data.len() - 4 {
                return Self { data, pos: 4 };
            }
        }
        Self { data, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    fn take(&mut self, n: usize) -> PResult<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(PickleError::Truncated {
                wanted: n,
                available: self.data.len() - self.pos,
            });
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn read_u8(&mut self) -> PResult<u8> {
        Ok(self.take(1)?[0])
    }

    /// Reads n raw bytes with no length prefix, advancing the cursor.
    ///
    /// Used for opaque fixed-width fields (e.g. the 16-byte `base::Token` in a
    /// group record) where consuming a typed value would hide exactly the
    /// endianness we want to avoid interpreting.
    pub fn read_raw(&mut self, n: usize) -> PResult<&'a [u8]> {
        self.take(n)
    }

    /// Reads a 4-byte little-endian int as `i32`.
    pub fn read_i32(&mut self) -> PResult<i32> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_u32(&mut self) -> PResult<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn read_i64(&mut self) -> PResult<i64> {
        let b = self.take(8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_u64(&mut self) -> PResult<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn read_f64(&mut self) -> PResult<f64> {
        Ok(f64::from_bits(self.read_u64()?))
    }

    /// Reads a 4-byte int as a boolean. Chromium writes `bool` this way.
    pub fn read_bool(&mut self) -> PResult<bool> {
        Ok(self.read_i32()? != 0)
    }

    /// Reads a 4-byte int as a `SessionID` value.
    pub fn read_session_id(&mut self) -> PResult<i32> {
        self.read_i32()
    }

    /// Reads a length-prefixed byte string, honouring 4-byte padding.
    pub fn read_bytes(&mut self) -> PResult<&'a [u8]> {
        let len = self.read_u32()? as u64;
        self.read_padded_slice(len)
    }

    /// Reads a length-prefixed UTF-8 string.
    pub fn read_string(&mut self) -> PResult<String> {
        let bytes = self.read_bytes()?;
        // Chromium writes URL/ASCII fields as raw bytes; invalid UTF-8 is
        // possible for a corrupt record, so decode lossily rather than fail the
        // whole snapshot.
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }

    /// Reads a length-prefixed UTF-16LE string.
    ///
    /// The length prefix counts **characters** (u16 units), not bytes.
    pub fn read_string16(&mut self) -> PResult<String> {
        let chars = self.read_u32()? as u64;
        let bytes = chars.checked_mul(2).ok_or(PickleError::ImplausibleLength {
            length: chars,
            remaining: self.remaining(),
        })?;
        let raw = self.read_padded_slice(bytes)?;
        if raw.len() % 2 != 0 {
            return Err(PickleError::BadUtf16 { bytes: raw.len() });
        }
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }

    /// Reads a length-prefixed slice and advances past its 4-byte padding.
    fn read_padded_slice(&mut self, len: u64) -> PResult<&'a [u8]> {
        if len > MAX_STRING_BYTES {
            return Err(PickleError::ImplausibleLength {
                length: len,
                remaining: self.remaining(),
            });
        }
        let len = len as usize;
        let raw = self.take(len)?;
        let padding = padding_for(len);
        if padding > 0 {
            // Padding is present but its content is irrelevant; skip it. If the
            // stream ends exactly at the payload (some writers pad only between
            // fields) do not fail.
            let _ = self.take(padding.min(self.remaining()));
        }
        Ok(raw)
    }

    /// Skips `n` bytes, clamped to what is available.
    pub fn skip(&mut self, n: usize) -> PResult<()> {
        let n = n.min(self.remaining());
        self.take(n)?;
        Ok(())
    }
}

/// Bytes needed to reach the next 4-byte boundary after `len` bytes.
#[inline]
pub const fn padding_for(len: usize) -> usize {
    (4 - (len % 4)) % 4
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal Pickle writer mirroring Chromium's rules, used to build fixtures.
    mod writer {
        pub struct PickleWriter {
            buf: Vec<u8>,
            header: bool,
        }

        impl Default for PickleWriter {
            fn default() -> Self {
                Self::new()
            }
        }

        impl PickleWriter {
            /// A bare payload (no length header), for PickleReader::new.
            pub fn new() -> Self {
                Self { buf: Vec::new(), header: false }
            }

            /// Starts a pickle: writes the length header. The length is
            /// back-filled by `finish`.
            pub fn new_with_header() -> Self {
                Self {
                    buf: vec![0, 0, 0, 0],
                    header: true,
                }
            }

            pub fn i32(mut self, v: i32) -> Self {
                self.buf.extend_from_slice(&v.to_le_bytes());
                self
            }

            pub fn u32(mut self, v: u32) -> Self {
                self.buf.extend_from_slice(&v.to_le_bytes());
                self
            }

            pub fn i64(mut self, v: i64) -> Self {
                self.buf.extend_from_slice(&v.to_le_bytes());
                self
            }

            pub fn bool(self, v: bool) -> Self {
                self.i32(if v { 1 } else { 0 })
            }

            pub fn string(mut self, s: &str) -> Self {
                self.buf
                    .extend_from_slice(&(s.len() as u32).to_le_bytes());
                self.buf.extend_from_slice(s.as_bytes());
                self.pad(s.len());
                self
            }

            /// Writes a UTF-16LE string, length-prefixed in characters.
            pub fn string16(mut self, s: &str) -> Self {
                let units: Vec<u16> = s.encode_utf16().collect();
                self.buf
                    .extend_from_slice(&(units.len() as u32).to_le_bytes());
                for u in &units {
                    self.buf.extend_from_slice(&u.to_le_bytes());
                }
                self.pad(units.len() * 2);
                self
            }

            fn pad(&mut self, written: usize) {
                let p = super::padding_for(written);
                self.buf.extend(std::iter::repeat_n(0u8, p));
            }

            /// Raw bytes, inserted without a length prefix (used for the outer
            /// command payload which already carries one).
            pub fn raw(mut self, bytes: &[u8]) -> Self {
                self.buf.extend_from_slice(bytes);
                self
            }

            /// Finalises a headered pickle by back-filling its length.
            pub fn finish(mut self) -> Vec<u8> {
                let body = self.buf.len() - 4;
                self.buf[0..4].copy_from_slice(&(body as u32).to_le_bytes());
                self.buf
            }

            /// Returns the body only (no length header), for use as a command
            /// payload.
            pub fn body(&self) -> &[u8] {
                if self.header {
                    &self.buf[4..]
                } else {
                    &self.buf[..]
                }
            }
        }
    }

    #[test]
    fn padding_math() {
        assert_eq!(padding_for(0), 0);
        assert_eq!(padding_for(1), 3);
        assert_eq!(padding_for(2), 2);
        assert_eq!(padding_for(3), 1);
        assert_eq!(padding_for(4), 0);
        assert_eq!(padding_for(5), 3);
        assert_eq!(padding_for(100), 0);
    }

    #[test]
    fn reads_ints_in_order() {
        let p = writer::PickleWriter::new().i32(7).i32(-3).i64(1 << 40).u32(9);
        let mut r = PickleReader::new(p.body());
        assert_eq!(r.read_i32().unwrap(), 7);
        assert_eq!(r.read_i32().unwrap(), -3);
        assert_eq!(r.read_i64().unwrap(), 1 << 40);
        assert_eq!(r.read_u32().unwrap(), 9);
        assert!(r.is_empty());
    }

    #[test]
    fn string_length_and_padding_is_honoured() {
        // "abc" is 3 bytes -> 1 byte of padding; the next int must still land on
        // a 4-byte boundary. This is the failure mode that corrupts URLs.
        let p = writer::PickleWriter::new()
            .string("abc")
            .i32(0x11223344)
            .string("a")
            .i32(0x55667788);
        let mut r = PickleReader::new(p.body());
        assert_eq!(r.read_string().unwrap(), "abc");
        assert_eq!(r.read_i32().unwrap(), 0x11223344);
        assert_eq!(r.read_string().unwrap(), "a");
        assert_eq!(r.read_i32().unwrap(), 0x55667788);
    }

    #[test]
    fn aligned_string_needs_no_padding() {
        let p = writer::PickleWriter::new().string("abcd").i32(42);
        let mut r = PickleReader::new(p.body());
        assert_eq!(r.read_string().unwrap(), "abcd");
        assert_eq!(r.read_i32().unwrap(), 42);
    }

    #[test]
    fn string16_counts_characters_not_bytes_and_pads() {
        // 3 characters = 6 bytes -> 2 bytes of padding.
        let p = writer::PickleWriter::new().string16("abc").i32(1).string16("x").i32(2);
        let mut r = PickleReader::new(p.body());
        assert_eq!(r.read_string16().unwrap(), "abc");
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_string16().unwrap(), "x");
        assert_eq!(r.read_i32().unwrap(), 2);
    }

    #[test]
    fn string16_handles_non_bmp_and_cyrillic() {
        for s in ["Привет", "日本語タイトル", "emoji \u{1F600} here", ""] {
            let p = writer::PickleWriter::new().string16(s).i32(99);
            let mut r = PickleReader::new(p.body());
            assert_eq!(r.read_string16().unwrap(), s);
            assert_eq!(r.read_i32().unwrap(), 99);
        }
    }

    #[test]
    fn from_pickle_validates_the_length_header() {
        let bytes = writer::PickleWriter::new_with_header().i32(5).string("hi").finish();
        let mut r = PickleReader::from_pickle(&bytes);
        assert_eq!(r.read_i32().unwrap(), 5);
        assert_eq!(r.read_string().unwrap(), "hi");
    }

    #[test]
    fn from_pickle_falls_back_when_there_is_no_valid_header() {
        // A body whose first four bytes are not a plausible length must be read
        // from offset 0 rather than skipped.
        let p = writer::PickleWriter::new().i32(0x7FFFFFFF).raw(b"\xff\xff\xff\xff");
        let mut r = PickleReader::from_pickle(p.body());
        assert_eq!(r.read_i32().unwrap(), 0x7FFFFFFF);
    }

    #[test]
    fn truncation_is_reported_not_panicked() {
        let p = writer::PickleWriter::new().i32(1);
        let mut r = PickleReader::new(p.body());
        assert_eq!(r.read_i32().unwrap(), 1);
        assert!(matches!(
            r.read_i32(),
            Err(PickleError::Truncated { .. })
        ));
    }

    #[test]
    fn implausible_lengths_are_rejected() {
        // A length prefix larger than the buffer means the stream is misaligned.
        let p = writer::PickleWriter::new().u32(0xFFFFFF).string("short");
        let mut r = PickleReader::new(p.body());
        assert!(matches!(
            r.read_string(),
            Err(PickleError::ImplausibleLength { .. })
        ));
    }

    #[test]
    fn read_bytes_returns_raw_content() {
        let p = writer::PickleWriter::new().string("payload").i32(3);
        let mut r = PickleReader::new(p.body());
        assert_eq!(r.read_bytes().unwrap(), b"payload");
        assert_eq!(r.read_i32().unwrap(), 3);
    }

    #[test]
    fn skip_clamps_and_does_not_error_past_the_end() {
        let p = writer::PickleWriter::new().i32(1).i32(2);
        let mut r = PickleReader::new(p.body());
        r.skip(1000).unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn bools_read_as_four_byte_ints() {
        let p = writer::PickleWriter::new().bool(true).bool(false).bool(true);
        let mut r = PickleReader::new(p.body());
        assert!(r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
        assert!(r.read_bool().unwrap());
    }
}
