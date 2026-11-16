//! Retention: deciding which snapshots to prune.
//!
//! The policy is expressed as three tiers, exactly as specified:
//!
//! * everything newer than `keep_all_hours` is kept;
//! * beyond that, one snapshot per **hour** is kept for `hourly_days`;
//! * beyond that, one snapshot per **day** is kept for `daily_days`;
//! * anything older is deleted.
//!
//! Recovery points (mass-close snapshots) are exempt by default — those are the
//! ones the user actually needs after an accident, so silently ageing them out
//! would defeat the purpose of the app.
//!
//! This module is deliberately a **pure function over metadata**. It returns the
//! list of ids to delete and never touches the database, which makes the policy
//! exhaustively testable from literal inputs instead of a populated DB.

use crate::model::{SnapshotMeta, SnapshotReason};
use crate::time::{bucket_start_ms, Bucket};

/// The retention policy, derived from `Settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub keep_all_hours: u32,
    pub hourly_days: u32,
    pub daily_days: u32,
    pub keep_recovery_points_forever: bool,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            keep_all_hours: 24,
            hourly_days: 30,
            daily_days: 365,
            keep_recovery_points_forever: true,
        }
    }
}

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

/// Outcome of a retention pass, with enough detail for the UI to explain itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetentionPlan {
    /// Snapshot ids to delete, ascending.
    pub delete: Vec<i64>,
    /// Snapshot ids kept because they are recovery points.
    pub protected: Vec<i64>,
    /// How many were kept by each tier, for the settings screen.
    pub kept_recent: usize,
    pub kept_hourly: usize,
    pub kept_daily: usize,
}

impl RetentionPlan {
    pub fn total_kept(&self) -> usize {
        self.kept_recent + self.kept_hourly + self.kept_daily + self.protected.len()
    }
}

/// Computes which snapshots to delete.
///
/// `snapshots` need not be sorted. Only `id`, `taken_ms`, `reason` and
/// `recovery_point` are consulted.
pub fn plan(snapshots: &[SnapshotMeta], policy: RetentionPolicy, now_ms: i64) -> RetentionPlan {
    let mut out = RetentionPlan::default();

    let recent_cutoff = now_ms - (policy.keep_all_hours as i64) * HOUR_MS;
    let hourly_cutoff = recent_cutoff - (policy.hourly_days as i64) * DAY_MS;
    let daily_cutoff = hourly_cutoff - (policy.daily_days as i64) * DAY_MS;

    // Group ids by the bucket they represent so each bucket keeps exactly one.
    // We keep the *newest* snapshot in each bucket: after an accident the last
    // state before things aged out is the most useful one.
    let mut hourly_best: std::collections::BTreeMap<i64, (i64, i64)> = std::collections::BTreeMap::new();
    let mut daily_best: std::collections::BTreeMap<i64, (i64, i64)> = std::collections::BTreeMap::new();
    for s in snapshots {
        if s.taken_ms > recent_cutoff {
            continue;
        }
        if s.taken_ms > hourly_cutoff {
            let b = bucket_start_ms(s.taken_ms, Bucket::Hour);
            let e = hourly_best.entry(b).or_insert((s.taken_ms, s.id));
            if s.taken_ms > e.0 {
                *e = (s.taken_ms, s.id);
            }
        } else if s.taken_ms > daily_cutoff {
            let b = bucket_start_ms(s.taken_ms, Bucket::Day);
            let e = daily_best.entry(b).or_insert((s.taken_ms, s.id));
            if s.taken_ms > e.0 {
                *e = (s.taken_ms, s.id);
            }
        }
    }

    let hourly_keep: std::collections::BTreeSet<i64> =
        hourly_best.values().map(|(_, id)| *id).collect();
    let daily_keep: std::collections::BTreeSet<i64> = daily_best.values().map(|(_, id)| *id).collect();

    for s in snapshots {
        let protected = policy.keep_recovery_points_forever
            && (s.recovery_point || s.reason == SnapshotReason::MassClose);
        if protected && s.taken_ms <= recent_cutoff {
            out.protected.push(s.id);
            continue;
        }
        if s.taken_ms > recent_cutoff {
            out.kept_recent += 1;
        } else if s.taken_ms > hourly_cutoff {
            if hourly_keep.contains(&s.id) {
                out.kept_hourly += 1;
            } else {
                out.delete.push(s.id);
            }
        } else if s.taken_ms > daily_cutoff {
            if daily_keep.contains(&s.id) {
                out.kept_daily += 1;
            } else {
                out.delete.push(s.id);
            }
        } else {
            out.delete.push(s.id);
        }
    }

    out.delete.sort_unstable();
    out.protected.sort_unstable();
    out
}

/// Visit statistics are aggregates, not events, so they are pruned by
/// importance rather than age: we drop domains the user visited once, long ago,
/// and keep anything with real usage.
pub fn visit_stat_is_worth_keeping(visit_count: i64, active_ms: i64, last_seen_ms: i64, now_ms: i64) -> bool {
    if visit_count >= 3 || active_ms >= 30_000 {
        return true;
    }
    // A single fleeting visit is kept for a year, then dropped.
    now_ms - last_seen_ms <= 365 * DAY_MS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SnapshotReason;

    fn meta(id: i64, taken_ms: i64) -> SnapshotMeta {
        SnapshotMeta {
            id,
            taken_ms,
            browser: "chrome".into(),
            profile: "Default".into(),
            fingerprint: format!("fp{id}"),
            tab_count: 1,
            window_count: 1,
            reason: SnapshotReason::Interval,
            recovery_point: false,
        }
    }

    fn meta_recovery(id: i64, taken_ms: i64) -> SnapshotMeta {
        SnapshotMeta {
            recovery_point: true,
            reason: SnapshotReason::MassClose,
            ..meta(id, taken_ms)
        }
    }

    #[test]
    fn keeps_everything_newer_than_the_recent_window() {
        let now = 1_000 * DAY_MS;
        let snaps: Vec<_> = (0..10).map(|i| meta(i + 1, now - i * 60_000)).collect();
        let p = plan(&snaps, RetentionPolicy::default(), now);
        assert!(p.delete.is_empty());
        assert_eq!(p.kept_recent, 10);
    }

    #[test]
    fn collapses_hourly_tier_to_one_per_hour() {
        let now = 0;
        let base = -(7 * DAY_MS); // inside the hourly tier (24h..30d)
        // 12 snapshots in one hour bucket, 12 in the next.
        let mut snaps = Vec::new();
        for i in 0..12 {
            snaps.push(meta(i + 1, base + i * 60_000));
            snaps.push(meta(i + 101, base + HOUR_MS + i * 60_000));
        }
        let p = plan(&snaps, RetentionPolicy::default(), now);
        assert_eq!(p.kept_hourly, 2, "exactly one snapshot per hour bucket");
        assert_eq!(p.delete.len(), 22);
        // The kept ones must be the newest in each bucket.
        assert!(p.delete.contains(&1) && !p.delete.contains(&12));
        assert!(p.delete.contains(&101) && !p.delete.contains(&112));
    }

    #[test]
    fn collapses_daily_tier_to_one_per_day() {
        let now = 0;
        let base = -(100 * DAY_MS); // inside the daily tier (30d..395d)
        let mut snaps = Vec::new();
        for day in 0..3 {
            for k in 0..5 {
                snaps.push(meta(day * 10 + k + 1, base + day * DAY_MS + k * 3_600_000));
            }
        }
        let p = plan(&snaps, RetentionPolicy::default(), now);
        assert_eq!(p.kept_daily, 3);
        assert_eq!(p.delete.len(), 12);
    }

    #[test]
    fn deletes_anything_beyond_the_daily_tier() {
        let now = 0;
        let s = vec![meta(1, -(400 * DAY_MS)), meta(2, -(366 * DAY_MS))];
        let p = plan(&s, RetentionPolicy::default(), now);
        // -400d is past the 395d horizon and must be deleted; -366d is inside
        // the daily tier, and as the only snapshot that day it survives.
        assert_eq!(p.delete, vec![1]);
        assert_eq!(p.kept_daily, 1);
        assert_eq!(p.total_kept(), 1);
    }

    #[test]
    fn recovery_points_survive_everything() {
        let now = 0;
        let s = vec![
            meta_recovery(1, -(900 * DAY_MS)),
            meta_recovery(2, -(400 * DAY_MS)),
            meta(3, -(400 * DAY_MS)),
        ];
        let p = plan(&s, RetentionPolicy::default(), now);
        assert_eq!(p.protected, vec![1, 2]);
        assert_eq!(p.delete, vec![3]);
    }

    #[test]
    fn recovery_points_can_opt_out_of_protection() {
        let now = 0;
        let s = vec![meta_recovery(1, -(900 * DAY_MS))];
        let p = plan(
            &s,
            RetentionPolicy {
                keep_recovery_points_forever: false,
                ..RetentionPolicy::default()
            },
            now,
        );
        assert_eq!(p.delete, vec![1]);
        assert!(p.protected.is_empty());
    }

    #[test]
    fn tier_boundaries_are_exclusive_at_the_cutoff() {
        // Exactly at the recent cutoff belongs to the hourly tier, not "recent".
        let now = 0;
        let cutoff = -(24 * HOUR_MS);
        // 191.5 h ago and 191.5 h + 1 min ago: both in the hourly tier and in
        // the SAME hour bucket.
        let same_hour = -(191 * HOUR_MS + HOUR_MS / 2);
        let s = vec![
            meta(1, cutoff + 1),      // just inside "recent"
            meta(2, same_hour),       // newer of the two -> kept
            meta(3, same_hour - 60_000), // older twin -> pruned
        ];
        let p = plan(&s, RetentionPolicy::default(), now);
        assert_eq!(p.kept_recent, 1);
        assert_eq!(p.kept_hourly, 1);
        assert_eq!(p.delete, vec![3]);
    }

    #[test]
    fn empty_input_is_a_no_op() {
        let p = plan(&[], RetentionPolicy::default(), 12345);
        assert_eq!(p, RetentionPlan::default());
        assert_eq!(p.total_kept(), 0);
    }

    #[test]
    fn long_history_is_pruned_to_a_bounded_size() {
        // ~3 years of minute-by-minute snapshots reduce to roughly
        // (24*60 recent) + (30*24 hourly) + (~365 daily); not everything is
        // kept, but the tier minimums must survive.
        let now = 2_000 * DAY_MS;
        let snaps: Vec<_> = (0..(3 * 365 * 24 * 60))
            .map(|i| meta(i as i64 + 1, now - (i as i64) * 60_000))
            .collect();
        let p = plan(&snaps, RetentionPolicy::default(), now);
        let kept = p.total_kept();
        assert!(kept >= 2_400, "kept {kept}, expected the tier minimums to survive");
        assert!(kept < 2_700, "kept {kept}, expected heavy pruning");
        assert_eq!(kept + p.delete.len(), snaps.len());
    }

    #[test]
    fn visit_stats_keep_meaningful_rows() {
        let now = 1_000 * DAY_MS;
        assert!(visit_stat_is_worth_keeping(3, 0, now - 900 * DAY_MS, now));
        assert!(visit_stat_is_worth_keeping(1, 30_000, now - 900 * DAY_MS, now));
        assert!(visit_stat_is_worth_keeping(1, 0, now - 10 * DAY_MS, now));
        assert!(!visit_stat_is_worth_keeping(1, 0, now - 400 * DAY_MS, now));
    }
}
