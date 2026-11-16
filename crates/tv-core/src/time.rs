//! Time handling.
//!
//! Two clocks appear throughout this codebase and mixing them up is the single
//! easiest way to produce a vault where everything sorts wrong:
//!
//! * **Unix epoch (ms)** — what `Date.now()` in the extension produces, what
//!   Firefox session files use, and what we store in SQLite.
//! * **Windows epoch (1601, µs)** — what Chromium's Pickle `datetime` and its
//!   `close_time` / `last_active_time` fields use.
//!
//! Every conversion is centralised here.

/// Seconds between the Windows epoch (1601-01-01) and the Unix epoch (1970-01-01).
pub const WINDOWS_TO_UNIX_EPOCH_SECS: i64 = 11_644_473_600;

/// Microseconds between the two epochs.
pub const WINDOWS_TO_UNIX_EPOCH_MICROS: i64 = WINDOWS_TO_UNIX_EPOCH_SECS * 1_000_000;

/// Current Unix time in milliseconds.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Converts a Chromium `base::Time` internal value (µs since 1601) to Unix ms.
///
/// Returns `None` for values that clearly are not real timestamps. Chromium
/// writes 0 for "unset", and garbage in a corrupt file can be wildly out of
/// range; both would otherwise sort to the top of a descending list.
pub fn windows_micros_to_unix_ms(micros: i64) -> Option<i64> {
    // Reject "unset" and anything before ~1970 or after the year 3000.
    const YEAR_3000_MS: i64 = 32_503_680_000_000;
    if micros <= 0 {
        return None;
    }
    let unix_ms = micros / 1000 - WINDOWS_TO_UNIX_EPOCH_MICROS / 1000;
    if !(0..=YEAR_3000_MS).contains(&unix_ms) {
        // Some third-party builds write a plain Unix timestamp into the same
        // field. Accept it when it is plausible as milliseconds.
        if (1_000_000_000_000..=YEAR_3000_MS).contains(&micros) {
            return Some(micros);
        }
        return None;
    }
    Some(unix_ms)
}

/// Converts a Chromium `base::Time` internal value (µs since 1601) where the
/// value is known to be a real timestamp; falls back to `now` for unset input.
pub fn windows_micros_to_unix_ms_or_now(micros: i64) -> i64 {
    windows_micros_to_unix_ms(micros).unwrap_or_else(now_ms)
}

/// Converts Unix milliseconds to the Chromium representation.
pub fn unix_ms_to_windows_micros(unix_ms: i64) -> i64 {
    (unix_ms + WINDOWS_TO_UNIX_EPOCH_MICROS / 1000) * 1000
}

/// Formats Unix ms as a local `YYYY-MM-DD HH:MM:SS` string for display.
pub fn format_local(unix_ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(unix_ms).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => "-".to_string(),
    }
}

/// Formats Unix ms as a local `YYYY-MM-DD` string.
pub fn format_local_date(unix_ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(unix_ms).single() {
        Some(dt) => dt.format("%Y-%m-%d").to_string(),
        None => "-".to_string(),
    }
}

/// Truncates Unix ms down to a UTC bucket boundary, used to build the
/// rolled-up retention tiers (hour / day) from a single stored timestamp.
pub fn bucket_start_ms(unix_ms: i64, bucket: Bucket) -> i64 {
    match bucket {
        Bucket::Minute => unix_ms - unix_ms.rem_euclid(60_000),
        Bucket::Hour => unix_ms - unix_ms.rem_euclid(3_600_000),
        Bucket::Day => unix_ms - unix_ms.rem_euclid(86_400_000),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Minute,
    Hour,
    Day,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_epoch_conversion_roundtrips() {
        let unix_ms = 1_700_000_000_000; // 2023-11-14T22:13:20Z
        let win = unix_ms_to_windows_micros(unix_ms);
        assert_eq!(windows_micros_to_unix_ms(win), Some(unix_ms));
    }

    #[test]
    fn windows_epoch_conversion_is_accurate() {
        // 2024-01-01T00:00:00Z == 1704067200000 ms unix
        let win_micros = (1_704_067_200i64 + WINDOWS_TO_UNIX_EPOCH_SECS) * 1_000_000;
        assert_eq!(windows_micros_to_unix_ms(win_micros), Some(1_704_067_200_000));
    }

    #[test]
    fn unset_and_garbage_timestamps_are_rejected() {
        assert_eq!(windows_micros_to_unix_ms(0), None);
        assert_eq!(windows_micros_to_unix_ms(-1), None);
        assert_eq!(windows_micros_to_unix_ms(i64::MAX), None);
        assert_eq!(windows_micros_to_unix_ms(i64::MIN), None);
    }

    #[test]
    fn unix_ms_written_into_a_windows_field_is_recovered() {
        // A handful of forks write plain unix ms here; we should not lose it.
        assert_eq!(windows_micros_to_unix_ms(1_700_000_000_000), Some(1_700_000_000_000));
    }

    #[test]
    fn buckets_floor_correctly() {
        let t = 1_700_000_123_456;
        assert_eq!(bucket_start_ms(t, Bucket::Minute), 1_700_000_100_000);
        assert_eq!(bucket_start_ms(t, Bucket::Hour), 1_699_999_200_000);
        assert_eq!(bucket_start_ms(t, Bucket::Day), 1_699_920_000_000);
        assert_eq!(bucket_start_ms(60_000, Bucket::Hour), 0);
    }
}
